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

typedef struct { uint8_t version[2], fingerprint[16], record[16], command[16], event[16]; } Schema;
typedef struct {
    uint8_t identity[104]; uint64_t index, term; uint8_t batch[32];
    uint64_t tx_seq; uint8_t chain[32];
} Position;
typedef struct {
    uint64_t tx_seq; uint32_t ordinal, reserved; uint8_t id[32], chain[32];
    uint8_t profile, reserved_profile[3]; uint16_t system, business; SvBytes digest;
    Schema target; uint8_t reserved_target[6]; uint64_t time, events, changes;
    Schema source; uint8_t reserved_source[6];
} Transaction;
typedef struct { Position position; uint64_t count; Schema schema; uint8_t reserved[6]; } State;
typedef struct {
    uint32_t tag, cause; uint64_t index, term, tx_seq; uint8_t chain[32];
    uint64_t count; Schema schema; uint8_t reserved[6];
} QueryInfo;
typedef struct {
    uint8_t header[32]; SvBytes target; void (*callbacks[8])(void);
    void (*decoder_idl)(void);
} Projector;
_Static_assert(sizeof(Schema) == 66, "schema extent");
_Static_assert(sizeof(Transaction) == 272 && offsetof(Transaction, source) == 200, "transaction schemas");
_Static_assert(sizeof(State) == 272 && offsetof(State, schema) == 200, "frozen source schema");
_Static_assert(sizeof(QueryInfo) == 144 && offsetof(QueryInfo, schema) == 72, "query schema");
_Static_assert(sizeof(Projector) == 120 && offsetof(Projector, decoder_idl) == 112, "mandatory decoder IDL");
