pub mod kexec_core;
pub mod syscall;

use crate::libs::spinlock::SpinLock;
use crate::mm::page::Page;
use alloc::sync::Arc;
use core::ffi::c_void;
use alloc::vec::Vec;
use core::arch::asm;

const KEXEC_SEGMENT_MAX: usize = 16;

pub static mut KEXEC_IMAGE: Option<Arc<SpinLock<Kimage>>> = None;

#[derive(Clone, Copy)]
#[repr(C)]
pub union kexec_segment_buf {
    pub buf: *mut c_void,  // For user memory (user space pointer)
    pub kbuf: *mut c_void, // For kernel memory (kernel space pointer)
}

#[derive(Clone, Copy)]
#[repr(C)]
pub struct KexecSegment {
    /// This pointer can point to user memory if kexec_load() system
    /// call is used or will point to kernel memory if
    /// kexec_file_load() system call is used.
    ///
    /// Use ->buf when expecting to deal with user memory and use ->kbuf
    /// when expecting to deal with kernel memory.
    pub buffer: kexec_segment_buf,
    pub bufsz: usize,
    pub mem: usize, // unsigned long typically matches usize
    pub memsz: usize,
}

/// kimage结构体定义, 没写全, 见https://code.dragonos.org.cn/xref/linux-6.1.9/include/linux/kexec.h#321
#[repr(C)]
pub struct Kimage {
    pub head: usize,
    pub entry_ptr: usize,
    pub last_entry_ptr: usize,

    pub start: usize,
    pub control_code_page: Option<Arc<Page>>,
    pub swap_page: Option<Arc<Page>>,
    pub vmcoreinfo_data_copy: usize,

    pub nr_segments: usize,
    pub segment: [KexecSegment; KEXEC_SEGMENT_MAX],

    pub ksegments_vec: Vec<Vec<u8>>,

    // TODO: 下面的页表是kimage_arch, 架构特化的
    /*
	 * This is a kimage control page, as it must not overlap with either
	 * source or destination address ranges.
	 */
    pub pgd: usize,

    /*
	 * The virtual mapping of the control code page itself is used only
	 * during the transition, while the current kernel's pages are all
	 * in place. Thus the intermediate page table pages used to map it
	 * are not control pages, but instead just normal pages obtained
	 * with get_zeroed_page(). And have to be tracked (below) so that
	 * they can be freed.
	 */
    pub p4d: usize,
    pub pud: usize,
    pub pmd: usize,
    pub pte: usize,


    /*
    /* Address of next control page to allocate for crash kernels. */
    unsigned long control_page;

    /* Flags to indicate special processing */
    unsigned int type : 1;
    #define KEXEC_TYPE_DEFAULT 0
    #define KEXEC_TYPE_CRASH   1.
    unsigned int preserve_context : 1;
    /* If set, we are using file mode kexec syscall */
    unsigned int file_mode:1;

    // Core ELF header buffer, used for KEXEC_CRASH
    pub elf_headers: usize,
    pub elf_headers_sz: usize,
    pub elf_load_addr: usize,
    */
}

bitflags! {
    pub struct KexecFlags: u64 {
        const KEXEC_ON_CRASH = 0x00000001;
        const KEXEC_PRESERVE_CONTEXT = 0x00000002;
        const KEXEC_ARCH_MASK = 0xffff0000;
    }
}

#[repr(C, packed)]
pub struct DescPtr {
    pub size: u16,
    pub address: u64, // 在 64 位系统中是 64 位地址
}

impl DescPtr {
    pub const fn new(address: u64, size: u16) -> Self {
        Self { size, address }
    }
}

#[inline]
pub unsafe fn native_load_gdt(dtr: &DescPtr) {
    unsafe {
        asm!(
            "lgdt [{}]",
            in(reg) dtr,
            options(nostack, preserves_flags)
        );
    }
}

#[inline(always)]
pub unsafe fn native_load_idt(dtr: &DescPtr) {
    unsafe {
        asm!(
            "lidt [{}]",
            in(reg) dtr,
            options(nostack, preserves_flags)
        );
    }
}

#[inline]
pub unsafe fn native_gdt_invalidate() {
    const INVALID_GDT: DescPtr = DescPtr::new(0, 0);
    native_load_gdt(&INVALID_GDT);
}

#[inline]
pub unsafe fn native_idt_invalidate() {
    const INVALID_IDT: DescPtr = DescPtr::new(0, 0);
    native_load_idt(&INVALID_IDT);
}
