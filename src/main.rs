#![no_main]
#![no_std]
#![feature(abi_efiapi)]

use core::fmt::Write;
use core::mem;
use uefi::prelude::*;
use uefi::table::boot::{MemoryDescriptor, MemoryType, AllocateType};
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType, FileInfo};
use uefi::CStr16;
use byteorder::{LittleEndian, ByteOrder};

#[macro_use]
extern crate alloc;

const KERNEL_BASE_ADDR: usize = 0x100000;
const EFI_PAGE_SIZE: usize = 0x1000;

#[entry]
fn main(image: Handle, mut system_table: SystemTable<Boot>) -> Status {
    uefi_services::init(&mut system_table).unwrap();
    writeln!(system_table.stdout(), "Hello world").unwrap();

    let mut root_dir = {
        let sfs = system_table.boot_services().get_image_file_system(image).unwrap().interface;
        unsafe {&mut *sfs.get()}.open_volume().unwrap()
    };

    writeln!(system_table.stdout(), "writing memory map...").unwrap();
    let mut file = match root_dir
        .open(CStr16::from_str_with_buf("memmap", &mut [0; 8]).unwrap(), 
            FileMode::CreateReadWrite, FileAttribute::empty())
        .unwrap()
        .into_type()
        .unwrap()
    {
        FileType::Regular(file) => file,
        FileType::Dir(_) => panic!(),
    };

    let size = system_table.boot_services().memory_map_size().map_size 
        + 8 * mem::size_of::<MemoryDescriptor>();
    let mut buf = vec![0u8; size];
    let (_, memory_descriptor) = system_table.boot_services().memory_map(&mut buf).unwrap();

    file.write("Index, Type, Type(name), PhysicalStart, \
        NumberOfPages, Attribute\n".as_bytes()).unwrap();

    for (i, d) in memory_descriptor.enumerate() {
        let line = format!(
            "{}, {:x}, {:?}, {:08x}, {:x}, {:x}\n", 
            i, d.ty.0, d.ty, d.phys_start, d.page_count, d.att.bits() & 0xfffff
        );
        file.write(line.as_bytes()).unwrap();
    }
    drop(file);
    writeln!(system_table.stdout(), "Done").unwrap();

    writeln!(system_table.stdout(), "Loading kernel...").unwrap();
    let mut file = match root_dir
        .open(CStr16::from_str_with_buf("kernel.elf", &mut [0; 12]).unwrap(),
            FileMode::CreateReadWrite, FileAttribute::empty())
        .unwrap()
        .into_type()
        .unwrap()
    {
        FileType::Regular(file) => file,
        FileType::Dir(_) => panic!(),
    };
    let size = file.get_boxed_info::<FileInfo>().unwrap().file_size() as usize;
    system_table.boot_services()
        .allocate_pages(
            AllocateType::Address(KERNEL_BASE_ADDR),
            MemoryType::LOADER_DATA,
            (size + EFI_PAGE_SIZE - 1) /EFI_PAGE_SIZE,
        )
        .unwrap();
    file.read(unsafe { core::slice::from_raw_parts_mut(KERNEL_BASE_ADDR as *mut u8, size) }).unwrap();
    drop(file);
    writeln!(system_table.stdout(), "Done").unwrap();

    // exit boot service
    system_table.exit_boot_services(image, &mut buf).unwrap();

    let entry_point_addr = LittleEndian::read_u64(unsafe {
        core::slice::from_raw_parts((KERNEL_BASE_ADDR + 24) as *const u8, 8)
    });
    //writeln!(system_table.stdout(), "{:x}", entry_point_addr).unwrap();
    let entry_point: extern "sysv64" fn() = unsafe { mem::transmute(entry_point_addr as usize) };
    entry_point();

    loop {}
}
