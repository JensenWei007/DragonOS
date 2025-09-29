/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * relocate_kernel.S - put the kernel image in place to boot
 * Copyright (C) Jingxuan Wei  <jensenwei007@gmail.com>
 */

#define X86_CR4_PGE		(1ul << 7)
#define X86_CR4_CET		(1ul << 23)
#define X86_CR0_AM          (1UL<<18)
#define X86_CR0_WP          (1UL<<16)
#define X86_CR0_TS          (1UL<<3)
#define X86_CR0_EM          (1UL<<2)
#define X86_CR0_PG          (1UL<<31)
#define X86_CR0_PE          (1UL<<0)
#define X86_CR4_PAE		(1ul << 5)
#define X86_CR4_LA57		(1ul << 12)

.section .relocate_kernel_data
.balign 16
kexec_debug_gdt:
    .word   kexec_debug_gdt_end - kexec_debug_gdt - 1
    .long   0
    .word   0
    .quad   0x00cf9a000000ffff      /* __KERNEL32_CS */
    .quad   0x00af9a000000ffff      /* __KERNEL_CS */
    .quad   0x00cf92000000ffff      /* __KERNEL_DS */
    .type kexec_debug_gdt_end 1 ;
kexec_debug_gdt_end:
    .type kexec_debug_gdt 1 ;
    .set .L__sym_size_kexec_debug_gdt, .-kexec_debug_gdt ;
    .size kexec_debug_gdt, .L__sym_size_kexec_debug_gdt

.globl kexec_pa_table_page
kexec_pa_table_page:
    .quad 0
    .type kexec_pa_table_page 1
    .set .L__sym_size_kexec_pa_table_page, .-kexec_pa_table_page
    .size kexec_pa_table_page, .L__sym_size_kexec_pa_table_page


.section .relocate_kernel_text
.code64
.globl relocate_kernel;   
relocate_kernel:
	/*
	 * %rdi const KexecSegment* segments
	 * %rsi usize nr_segments
	 * %rdx usize start address
	 */

	/* zero out flags, and disable interrupts */
	#pushq $0
	#popfq

	# 保存寄存器
    push %rbp
    mov %rsp, %rbp
    push %rbx
	push %rdx
    push %r12
    push %r13
    push %r14
    push %r15

    #int3 ok 00f

    /* Switch to the identity mapped page tables */
	movq	%cr3, %rax
    #int3 ok 012
	movq	kexec_pa_table_page(%rip), %r9
    #int3 ok 019
	movq	%r9, %cr3
    

    /* Leave CR4 in %r13 to enable the right paging mode later. */
	movq	%cr4, %r13

    #int3 ok 00f
	/* Disable global pages immediately to ensure this mapping is RWX */
	movq	%r13, %r12
	andq	$~(X86_CR4_PGE), %r12
	movq	%r12, %cr4

    #int3 ok 021

	/* Create a GDTR (16 bits limit, 64 bits addr) on stack */
	#leaq	kexec_debug_gdt(%rip), %rax
	# int3 ok
	#pushq	%rax
	# int3 ok 17
	#pushw	(%rax)

	/* Load the GDT, put the stack back */
	#int3 ok 1a
	#lgdt	(%rsp)
	#int3
	#addq	$10, %rsp

	/* Test that we can load segments */
	movq	%ds, %rax
	movq	%rax, %ds

    #int3 ok 027

    /*
	 * Clear X86_CR4_CET (if it was set) such that we can clear CR0_WP
	 * below.
	 */
	movq	%cr4, %rax
	andq	$~(X86_CR4_CET), %rax
	movq	%rax, %cr4

    #int3 ok 033
    /*
	 * Set cr0 to a known state:
	 *  - Paging enabled
	 *  - Alignment check disabled
	 *  - Write protect disabled
	 *  - No task switch
	 *  - Don't do FP software emulation.
	 *  - Protected mode enabled
	 */
	movq	%cr0, %rax
	andq	$~(X86_CR0_AM | X86_CR0_WP | X86_CR0_TS | X86_CR0_EM), %rax
	orl	$(X86_CR0_PG | X86_CR0_PE), %eax
	movq	%rax, %cr0

    #int3 ok 044
    /*
	 * Set cr4 to a known state:
	 *  - physical address extension enabled
	 *  - 5-level paging, if it was enabled before
	 *  - Machine check exception on TDX guest, if it was enabled before.
	 *    Clearing MCE might not be allowed in TDX guests, depending on setup.
	 *
	 * Use R13 that contains the original CR4 value, read in relocate_kernel().
	 * PAE is always set in the original CR4.
	 */
	andl	$(X86_CR4_PAE | X86_CR4_LA57), %r13d
	movq	%r13, %cr4

    #int3 ok 04f
        
    # 设置循环
    mov %rdi, %rbx        # rbx = 当前段指针
    mov %rsi, %r12        # r12 = 总段数
    xor %r13, %r13        # r13 = 当前索引

.process_segment:
    # 加载段信息
    mov 0x00(%rbx), %rdi  # rdi = source (buffer.buf)
    mov 0x08(%rbx), %rsi  # rsi = size (bufsz)
    mov 0x10(%rbx), %rdx  # rdx = destination (mem)
    mov 0x18(%rbx), %rcx  # rcx = destination size (memsz)
    
    # 参数验证
    test %rdi, %rdi       # 检查源指针是否为空
    jz .skip_segment
    
    test %rsi, %rsi       # 检查源大小是否为0
    jz .skip_segment
    
    test %rdx, %rdx       # 检查目标指针是否为空
    jz .skip_segment
    
    test %rcx, %rcx       # 检查目标大小是否为0
    jz .skip_segment
        
    # 执行内存复制
    call memory_copy
        
.skip_segment:
    # 移动到下一个段
    add $32, %rbx
    inc %r13
    cmp %r13, %r12
    jl .process_segment
    
    # 成功返回
    xor %rax, %rax
    jmp .exit_success

.exit_success:
    # 恢复寄存器
    pop %r15
    pop %r14
    pop %r13
    pop %r12
	pop %rdx
    pop %rbx
    pop %rbp

	/* store the start address on the stack */
	pushq   %rdx

	/*
	 * set all of the registers to known values
	 * leave %rsp alone
	 */

	xorl	%eax, %eax
	xorl	%ebx, %ebx
	xorl    %ecx, %ecx
	xorl    %edx, %edx
	xorl    %esi, %esi
	xorl    %edi, %edi
	xorl    %ebp, %ebp
	xorl	%r8d, %r8d
	xorl	%r9d, %r9d
	xorl	%r10d, %r10d
	xorl	%r11d, %r11d
	xorl	%r12d, %r12d
	xorl	%r13d, %r13d
	xorl	%r14d, %r14d
	xorl	%r15d, %r15d

	# int 089

    ret
	int3

.type relocate_kernel 0 ;
.set .L__sym_size_relocate_kernel, .-relocate_kernel ;
.size relocate_kernel, .L__sym_size_relocate_kernel

# void* memory_copy(void* dest, void* src, size_t n)
memory_copy:
	# rdi = src, rsi = n, rdx = dest
    # 注意：这里我们调整参数顺序
    
    mov %rdi, %r8         # r8 = src (保存源指针)
    mov %rsi, %rcx        # rcx = n (复制大小)
    mov %rdx, %rdi        # rdi = dest (目标指针)
    mov %r8, %rsi         # rsi = src (源指针)
    
    # 优化：检查大小并选择复制策略
    test %rcx, %rcx
    jz .copy_done
    
    # 检查对齐情况
    mov %rdi, %rax
    or %rsi, %rax
    test $7, %rax         # 检查是否8字节对齐
    jnz .byte_copy        # 如果不对齐，使用字节复制
    
    # 8字节对齐复制
.quad_copy:
    mov %rcx, %r9
    shr $3, %r9           # r9 = 8字节块的数量
    jz .remainder_copy
    
.quad_loop:
    mov (%rsi), %rax
    mov %rax, (%rdi)
    add $8, %rsi
    add $8, %rdi
    dec %r9
    jnz .quad_loop
    
.remainder_copy:
    and $7, %rcx          # 剩余字节数
    jz .copy_done

.byte_copy:
    # 字节复制剩余部分
    mov (%rsi), %al
    mov %al, (%rdi)
    inc %rsi
    inc %rdi
    dec %rcx
    jnz .byte_copy

.copy_done:
    mov %rdx, %rax        # 返回目标指针
    ret
.type memory_copy 0 ;
.set .L__sym_size_memory_copy, .-memory_copy ;
.size memory_copy, .L__sym_size_memory_copy
