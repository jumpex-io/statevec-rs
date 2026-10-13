/* Independent C11 layout oracle for the public record-change ABI. */
#include <stddef.h>
#include <stdint.h>

typedef struct {
    const uint8_t *ptr;
    uint64_t len;
} SvBytes;

typedef struct {
    uint16_t kind;
    uint16_t reserved16;
    uint32_t reserved32;
    uint64_t sys_id;
} SvKey;

typedef struct {
    SvKey key;
    uint64_t revision;
    uint32_t operation;
    uint32_t reserved;
    SvBytes before;
    SvBytes after;
} SvRecordChange;

_Static_assert(sizeof(SvBytes) == 16, "bytes extent");
_Static_assert(sizeof(SvKey) == 16, "key extent");
_Static_assert(sizeof(SvRecordChange) == 64, "record change extent");
_Static_assert(_Alignof(SvRecordChange) == 8, "record change alignment");
_Static_assert(offsetof(SvRecordChange, revision) == 16, "revision offset");
_Static_assert(offsetof(SvRecordChange, operation) == 24, "operation offset");
_Static_assert(offsetof(SvRecordChange, before) == 32, "before offset");
_Static_assert(offsetof(SvRecordChange, after) == 48, "after offset");
