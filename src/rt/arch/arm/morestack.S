// Mark stack as non-executable
#if defined(__linux__) && defined(__ELF__)
.section	.note.GNU-stack, "", %progbits
#endif

/* See i386/morestack.S for the lengthy, general explanation. */

.text
.code 32
.arm
#if defined(__APPLE__)
.align 2
#else
.align
#endif

#if defined(__APPLE__)
#define MORESTACK ___morestack
#else
#define MORESTACK __morestack
#endif

.global MORESTACK

// Unfortunately LLVM yet doesn't support emitting correct debug
// DWARF information for non-ELF targets so to make it compile
// on iOS all that directives are simply commented out
#if defined(__APPLE__)
#define UNWIND @
#else
#define UNWIND
#endif

#if defined(__APPLE__)
.private_extern MORESTACK
#else
.hidden MORESTACK
#endif

#if !defined(__APPLE__)
   .type MORESTACK,%function
#endif

// r4 and r5 are scratch registers for __morestack due to llvm
// ARMFrameLowering::adjustForSegmentedStacks() implementation.
MORESTACK:
    UNWIND .fnstart
    b   MORESTACK
    UNWIND  .fnend
