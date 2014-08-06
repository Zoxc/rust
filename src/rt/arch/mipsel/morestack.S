// Mark stack as non-executable
#if defined(__linux__) && defined(__ELF__)
.section        .note.GNU-stack, "", @progbits
#endif

/* See i386/morestack.S for the lengthy, general explanation. */

.text

.globl __morestack

.hidden __morestack

.cfi_startproc
.set nomips16
.ent __morestack
__morestack:
        .set noreorder
        .set nomacro

        b       __morestack
        nop

.end __morestack
.cfi_endproc
