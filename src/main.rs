#![no_main]
#![no_std]
#![feature(abi_efiapi)]

use core::fmt::Write;
use core::mem;
use uefi::prelude::*;
use uefi::table::boot::MemoryDescriptor;
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType};
use uefi::CStr16;

#[macro_use]
extern crate alloc;

#[entry]
fn main(image: Handle, mut system_table: SystemTable<Boot>) -> Status {
    uefi_services::init(&mut system_table).unwrap();
    writeln!(system_table.stdout(), "Hello world").unwrap();

    let mut root_dir = {
        let sfs = system_table.boot_services().get_image_file_system(image).unwrap().interface;
        unsafe {&mut *sfs.get()}.open_volume().unwrap()
    };
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
    loop {}
}
