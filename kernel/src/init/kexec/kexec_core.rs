use super::{kexec_segment_buf, KexecFlags, KexecSegment, Kimage, KEXEC_IMAGE};
use crate::arch::mm::LockedFrameAllocator;
use crate::arch::CurrentIrqArch;
use crate::arch::MMArch;
use crate::exception::InterruptArch;
use crate::libs::spinlock::SpinLock;
use crate::mm::ident_map::{ident_map, ident_pt_alloc};
use crate::mm::kernel_mapper::KernelMapper;
use crate::mm::page::{
    page_manager_lock_irqsave, round_down_to_page_size, Page, PageFlags, PageType,
};
use crate::mm::VirtAddr;
use crate::mm::{page::EntryFlags, MemoryManagementArch, PhysAddr};
use crate::syscall::user_access::UserBufferReader;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::mem::transmute;
use system_error::SystemError;

pub fn do_kexec_load(
    entry: usize,
    nr_segments: usize,
    ksegments: &[KexecSegment],
    flags: usize,
) -> Result<usize, SystemError> {
    let _flags = KexecFlags::from_bits_truncate(flags as u64);

    if nr_segments == 0 {
        /* Uninstall image */
        log::warn!("kexec: nr_segments == 0");
        return Ok(0);
    }

    let image = kimage_alloc_init(entry, nr_segments, ksegments, flags).unwrap();

    image.lock().ksegments_vec = Vec::with_capacity(nr_segments);

    for i in 0..nr_segments {
        let segment = image.lock().segment[i].clone();

        let usegments_buf = UserBufferReader::new::<u8>(
            unsafe { segment.buffer.buf } as *mut u8,
            core::mem::size_of::<u8>() * segment.bufsz,
            true,
        )
        .unwrap();
        let ksegment: &[u8] = usegments_buf.read_from_user(0).unwrap();
        let mut ksegment_vec = ksegment.to_vec();
        // 目前正常情况不会出现大于，因为是把他扩大到页对齐
        if ksegment_vec.len() < segment.memsz {
            ksegment_vec.resize(segment.memsz, 0);
        }
        image.lock().segment[i].buffer.buf = unsafe {
            MMArch::virt_2_phys(VirtAddr::new(ksegment_vec.as_ptr() as usize))
                .unwrap()
                .data()
        } as *mut c_void;
        image.lock().ksegments_vec.push(ksegment_vec);
    }

    /*
    for i in 0..nr_segments {
        let mem = image.lock().segment[i].clone().mem;
        let memsz = image.lock().segment[i].clone().memsz;
        let virt = unsafe { MMArch::phys_2_virt(PhysAddr::new(mem)).unwrap() };
        let mut kernel_mapper = KernelMapper::lock();
        unsafe {
            kernel_mapper
                .map_phys_with_size(
                    virt,
                    PhysAddr::new(mem),
                    memsz,
                    EntryFlags::new().set_execute(true).set_write(true),
                    true,
                )
                .unwrap()
        };
        image.lock().segment[i].mem = virt.data();
    }
    */

    init_pgtable(image.clone());

    if !machine_kexec_prepare(image.clone()) {
        return Err(SystemError::EADV);
    }

    unsafe {
        KEXEC_IMAGE = Some(image.clone());
    }

    log::info!("do load end");

    Ok(0)
}

pub fn kimage_alloc_init(
    entry: usize,
    nr_segments: usize,
    ksegments: &[KexecSegment],
    flags: usize,
) -> Result<Arc<SpinLock<Kimage>>, SystemError> {
    let image = Arc::new(SpinLock::new(Kimage {
        head: 0,
        entry_ptr: 0,
        last_entry_ptr: 0,
        start: 0,
        control_code_page: None,
        swap_page: None,
        vmcoreinfo_data_copy: 0,
        nr_segments: 0,
        segment: [KexecSegment {
            buffer: kexec_segment_buf {
                buf: core::ptr::null_mut(),
            },
            bufsz: 0,
            mem: 0,
            memsz: 0,
        }; super::KEXEC_SEGMENT_MAX],
        ksegments_vec: Vec::new(),
        pgd: 0,
        p4d: 0,
        pud: 0,
        pmd: 0,
        pte: 0,
    }));

    image.lock().start = entry;
    image.lock().nr_segments = nr_segments;

    for i in 0..ksegments.len() {
        image.lock().segment[i] = ksegments[i].clone();
    }

    let temp = kimage_alloc_control_pages(image.clone(), 1);
    image.lock().control_code_page = temp.clone();

    Ok(image)
}

pub fn kimage_alloc_control_pages(
    kimage: Arc<SpinLock<Kimage>>,
    order: usize,
) -> Option<Arc<Page>> {
    let mut page = None;
    let mut extra_pages: Vec<Arc<Page>> = Vec::new();
    let mut alloc = page_manager_lock_irqsave();

    let count = 1 << order;

    loop {
        let mut pfn = 0;
        let mut epfn = 0;
        let mut addr = 0;
        let mut eaddr = 0;

        let p = alloc
            .create_one_page(
                PageType::Normal,
                PageFlags::PG_RESERVED | PageFlags::PG_PRIVATE,
                &mut LockedFrameAllocator,
            )
            .unwrap();

        if check_isdst(kimage.clone(), p.clone()) {
            extra_pages.push(p);
            continue;
        }
        page = Some(p.clone());
        break;
    }

    for p in extra_pages {
        alloc.remove_page(&p.phys_address());
    }

    page
}

pub fn check_isdst(kimage: Arc<SpinLock<Kimage>>, page: Arc<Page>) -> bool {
    let nr_segments = unsafe { kimage.lock().nr_segments };
    let segments = unsafe { kimage.lock().segment };
    let paddr = page.phys_address().data();

    for i in 0..nr_segments {
        let mem = segments[i].mem;
        let memend = mem + segments[i].memsz;
        if paddr >= mem && paddr <= memend {
            return true;
        }
    }

    false
}

// TODO: 应该移到arch下，每个架构是独特的
pub fn init_pgtable(kimage: Arc<SpinLock<Kimage>>) {
    let pgd = ident_pt_alloc();
    kimage.lock().pgd = pgd;

    unsafe extern "C" {
        pub unsafe static mut kexec_pa_table_page: u64;
    }

    unsafe {
        kexec_pa_table_page = pgd as u64;
        log::info!("init_pgtable, pgd:{:#x}, p:{:#x}", pgd, kexec_pa_table_page);
    }

    let nr_segments = unsafe { kimage.lock().nr_segments };

    // kimage.segment
    let addr = unsafe {
        MMArch::virt_2_phys(VirtAddr::new(kimage.lock().segment.as_ptr() as usize))
            .unwrap()
            .data()
    };
    let addr_b = round_down_to_page_size(addr);
    let addr_t = round_down_to_page_size(addr + 32 * 16);
    if addr_b == addr_t {
        ident_map(pgd, addr_b, addr_b);
    } else {
        ident_map(pgd, addr_b, addr_b);
        ident_map(pgd, addr_t, addr_t);
    }

    // segments
    for i in 0..nr_segments {
        let mut addr = unsafe { kimage.lock().segment[i].buffer.buf } as usize;
        let mut size = kimage.lock().segment[i].memsz;

        loop {
            ident_map(pgd, addr, addr);
            addr += 4096;
            size -= 4096;

            // 之前已经进行了页对齐
            if size == 0 {
                break;
            }
        }
    }

    // mems
    for i in 0..nr_segments {
        let mut addr = kimage.lock().segment[i].mem;
        let mut size = kimage.lock().segment[i].memsz;

        loop {
            ident_map(pgd, addr, addr);
            addr += 4096;
            size -= 4096;

            // 之前已经进行了页对齐
            if size == 0 {
                break;
            }
        }
    }

    // efi
    // map_efi_systab()

    // ACPI
    // map_acpi_tables()

    // control_page
    let control_page_pa: usize = kimage
        .lock()
        .control_code_page
        .clone()
        .unwrap()
        .phys_address()
        .data();
    ident_map(pgd, control_page_pa, control_page_pa);
}

// TODO: 应该移到arch下，每个架构是独特的
pub fn machine_kexec_prepare(kimage: Arc<SpinLock<Kimage>>) -> bool {
    unsafe {
        unsafe extern "C" {
            unsafe fn __relocate_kernel_start();
            unsafe fn __relocate_kernel_end();
        }
        let reloc_start = __relocate_kernel_start as usize;
        let reloc_end = __relocate_kernel_end as usize;

        log::info!("__relocate_kernel_end {:#x}", reloc_end);

        if reloc_end - reloc_start > 4096 {
            panic!("Kexec: relocate_kernel func is bigger than PAGE_SIZE");
        }

        let control_page_phys = kimage
            .lock()
            .control_code_page
            .clone()
            .unwrap()
            .phys_address();
        let virt = MMArch::phys_2_virt(control_page_phys).unwrap().data();
        log::info!("copy control from {:#x} to {:#x}", reloc_start, virt);

        core::ptr::copy(
            reloc_start as *mut u8,
            virt as *mut u8,
            reloc_end - reloc_start,
        );
    }
    true
}

type RelocateKernelFn =
    unsafe extern "C" fn(segments: usize, nr_segments: usize, start_address: usize) -> usize;

const __KERNEL_DS: u64 = 24;
unsafe fn load_segments() {
    core::arch::asm!(
        "mov ds, rax",
        "mov es, rax",
        "mov ss, rax",
        "mov fs, rax",
        "mov gs, rax",
        in("rax") __KERNEL_DS,
        options(nostack, nomem)
    );
}

pub fn kernel_kexec() {
    unsafe {
        log::info!("staret will rela");
        if KEXEC_IMAGE.is_none() {
            return ();
        }
        CurrentIrqArch::interrupt_disable();

        let kimage = KEXEC_IMAGE.clone().unwrap().clone();

        unsafe extern "C" {
            unsafe fn relocate_kernel();
            unsafe fn __relocate_kernel_start();
        }

        let control_page_virt = MMArch::phys_2_virt(
            kimage
                .lock()
                .control_code_page
                .clone()
                .unwrap()
                .phys_address(),
        )
        .unwrap()
        .data();
        log::info!(
            "__relocate_kernel_start {:#x}, relocate_kernel {:#x}",
            __relocate_kernel_start as usize,
            relocate_kernel as usize
        );
        let mut relocate_kernel_ptr: usize =
            (control_page_virt + relocate_kernel as usize - __relocate_kernel_start as usize);
        log::info!("relocate_kernel_ptr:{:#x}", relocate_kernel_ptr as usize);

        //let relocate_kernel_func: &unsafe fn(usize, usize, usize) -> usize = &*relocate_kernel_ptr;
        let relocate_kernel_func: RelocateKernelFn = unsafe { transmute(relocate_kernel_ptr) };

        let arg1 = unsafe {
            MMArch::virt_2_phys(VirtAddr::new(kimage.lock().segment.as_ptr() as usize))
                .unwrap()
                .data()
        };
        let arg2 = kimage.lock().nr_segments;
        let arg3 = kimage.lock().start;
        log::info!("staret will rela----------------");
        //load_segments();
        log::info!("staret will rela----------------2222222");
        //super::native_idt_invalidate();
        //super::native_gdt_invalidate();

        log::info!("will rela");

        unsafe {
            core::arch::asm!(
                "2:",
                "jmp 2b",
                options(noreturn)
            );
        }

        let _r = relocate_kernel_func(arg1, arg2, arg3);

        panic!("-----------------");
    }
}
