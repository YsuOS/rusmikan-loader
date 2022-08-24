#![no_main]
#![no_std]
#![feature(abi_efiapi)]
#![feature(vec_into_raw_parts)]

use alloc::vec::Vec;
use core::fmt::Write;
use core::mem;
use uefi::prelude::*;
use uefi::table::boot::{MemoryDescriptor, MemoryType, AllocateType};
use uefi::proto::media::file::{File, FileAttribute, FileMode, FileType, FileInfo};
use uefi::CStr16;
use uefi::table::Runtime;
use byteorder::{LittleEndian, ByteOrder};
use uefi::proto::console::gop::{GraphicsOutput, PixelFormat};
use rusmikan::{FrameBuffer,FrameBufferConfig};
use goblin::elf;

#[macro_use]
extern crate alloc;

const UEFI_PAGE_SIZE: usize = 0x1000;

fn dump_memory_map(image: Handle, system_table: &SystemTable<Boot>) {
    let mut root_dir = {
        let sfs = system_table.boot_services().get_image_file_system(image).unwrap().interface;
        unsafe {&mut *sfs.get()}.open_volume().unwrap()
    };
    //writeln!(system_table.stdout(), "writing memory map...").unwrap();
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

    let mmap_size = system_table.boot_services().memory_map_size().map_size 
        + 8 * mem::size_of::<MemoryDescriptor>();
    let mut mmap_buf = vec![0u8; mmap_size];
    let (_, memory_descriptor) = system_table.boot_services().memory_map(&mut mmap_buf).unwrap();

    file.write("Index, Type, Type(name), PhysicalStart, \
        NumberOfPages, Attribute\n".as_bytes()).unwrap();
    for (i, d) in memory_descriptor.enumerate() {
        let line = format!(
            "{}, {:x}, {:?}, {:08x}, {:x}, {:x}\n", 
            i, d.ty.0, d.ty, d.phys_start, d.page_count, d.att.bits() & 0xfffff
        );
        file.write(line.as_bytes()).unwrap();
    }
}

fn get_frame_buffer_config(system_table: &SystemTable<Boot>) -> FrameBufferConfig {
    let gop = system_table.boot_services().locate_protocol::<GraphicsOutput>().unwrap();
    let gop = unsafe { &mut *gop.get() };
    let fb_info = gop.current_mode_info();
    let (hori, vert) = fb_info.resolution();
    let pixels_per_scan_line = fb_info.stride();
    let pixel_format = match fb_info.pixel_format() {
        PixelFormat::Rgb => rusmikan::PixelFormat::RGB,
        PixelFormat::Bgr => rusmikan::PixelFormat::BGR,
        _ => panic!(),
    };
    let mut fb = gop.frame_buffer();
    let fb_ptr = fb.as_mut_ptr();
    FrameBufferConfig {
        frame_buffer: FrameBuffer{base: fb_ptr},
        horizontal_resolution: hori,
        vertical_resolution: vert,
        pixels_per_scan_line,
        pixel_format,
    }
}

fn load_elf(src: &[u8], system_table: &SystemTable<Boot>) -> u64 {
    let elf = elf::Elf::parse(&src).expect("Failed to parse elf file");
    
    let mut dest_first = usize::MAX;
    let mut dest_last = 0;
    for ph in elf.program_headers.iter() {
        if ph.p_type != elf::program_header::PT_LOAD {
            continue;
        }
        dest_first = dest_first.min(ph.p_vaddr as usize);
        dest_last = dest_last.max((ph.p_vaddr + ph.p_memsz) as usize);
    }

    system_table.boot_services()
        .allocate_pages(
            AllocateType::Address(dest_first),
            MemoryType::LOADER_DATA,
            (dest_last - dest_first + UEFI_PAGE_SIZE - 1) / UEFI_PAGE_SIZE,
        )
        .expect("failed to allocate pages for kernel");

    for ph in elf.program_headers.iter() {
        if ph.p_type != elf::program_header::PT_LOAD {
            continue;
        }
        let ofs = ph.p_offset as usize;
        let fsize = ph.p_filesz as usize;
        let msize = ph.p_memsz as usize;
        let dest = unsafe { core::slice::from_raw_parts_mut(ph.p_vaddr as *mut u8, msize) };
        dest[..fsize].copy_from_slice(&src[ofs..ofs + fsize]);
        dest[fsize..].fill(0);
    }

    LittleEndian::read_u64(unsafe {
        core::slice::from_raw_parts((dest_first + 24) as *const u8, 8)
    })
}

fn load_kernel(image: Handle, system_table: &SystemTable<Boot>) -> u64 {
    let mut root_dir = {
        let sfs = system_table.boot_services().get_image_file_system(image).unwrap().interface;
        unsafe {&mut *sfs.get()}.open_volume().unwrap()
    };
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
    let mut buf = vec![0; size];
    file.read(&mut buf).unwrap();

    load_elf(&buf, system_table)
}

fn exit_boot_services(image: Handle, system_table: SystemTable<Boot>)
    -> (SystemTable<Runtime>, rusmikan::MemoryMap) {
     let mmap_size = system_table.boot_services().memory_map_size().map_size 
        + 8 * mem::size_of::<MemoryDescriptor>();
    let mut mmap_buf = vec![0; mmap_size].leak();
    let mut descriptors = Vec::with_capacity(mmap_size);
    let (system_table, memory_descriptors) = system_table.exit_boot_services(image, &mut mmap_buf).unwrap();        
    for d in memory_descriptors {
        if is_available_after_exit_boot_services(d.ty) {
            descriptors.push(rusmikan::MemoryDescriptor {
                phys_start: d.phys_start,
            }) 
        }        
    }
    let memory_map = {
        let (ptr, len, _) = descriptors.into_raw_parts();
        rusmikan::MemoryMap {
            descriptors: ptr,
            descriptor_len: len as u64
        }
    };
    (system_table, memory_map)
}

fn is_available_after_exit_boot_services(ty: MemoryType) -> bool {
    matches!(
        ty,
        MemoryType::CONVENTIONAL | MemoryType::BOOT_SERVICES_CODE | MemoryType::BOOT_SERVICES_DATA
    )
}


#[entry]
fn main(image: Handle, mut system_table: SystemTable<Boot>) -> Status {
    uefi_services::init(&mut system_table).unwrap();
    //writeln!(system_table.stdout(), "Hello world").unwrap();

    // Write memory map to file
    dump_memory_map(image, &system_table);

    // Get frame buffer config
    let config = get_frame_buffer_config(&system_table);
    
    // Load elf image into memory
    let entry_point_addr = load_kernel(image, &system_table);
    writeln!(system_table.stdout(), "{:x}", entry_point_addr).unwrap();
    let entry_point: extern "sysv64" fn(&FrameBufferConfig, &rusmikan::MemoryMap) = unsafe { mem::transmute(entry_point_addr as usize) };
   
    // exit boot service and retrieve memory map to own MemoryDescriptor
    let (_, memory_map) = exit_boot_services(image, system_table);

    entry_point(&config, &memory_map);

    loop {}
}
