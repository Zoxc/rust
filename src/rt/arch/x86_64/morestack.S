// Mark stack as non-executable
#if defined(__linux__) && defined(__ELF__)
.section	.note.GNU-stack, "", @progbits
#endif

/* See i386/morestack.S for the lengthy, general explanation. */

.text

#if defined(__APPLE__)
#define MORESTACK               ___morestack
#else
#define MORESTACK               __morestack
#endif

#if defined(__linux__) || defined(__FreeBSD__) || defined(__DragonFly__)
	.hidden MORESTACK
#else
#if defined(__APPLE__)
	.private_extern MORESTACK
#endif
#endif

#ifdef __ELF__
	.type MORESTACK,@function
#endif

.globl MORESTACK
MORESTACK:
	.cfi_startproc
	jmp MORESTACK
	.cfi_endproc
