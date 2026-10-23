//! One definition of each scalar vocabulary; raw DTOs still use fixed u32s.
macro_rules! vocabulary {
    ($module:ident { $($name:ident = $value:literal),+ $(,)? }) => {
        pub mod $module { $(pub const $name: u32 = $value;)+ }
    };
}
vocabulary!(receive_tag { FRAME=0, EMPTY=1, DISCONNECTED=2, STOP=3, TIMED_OUT=4, STATE=5 });
vocabulary!(offer_tag { QUEUED=0, OWNER_QUEUE_FULL=1, CLOSED=2, REPLY_CAPACITY_EXHAUSTED=3 });
vocabulary!(state_tag { RECORD=0, END=1, STOP=2, PROGRESS=3 });
vocabulary!(reply_tag { PENDING=0, ACCEPTED=1, REJECTED=2, STATUS=3, DISCONNECTED=4 });
vocabulary!(status_kind { DISABLED=0, REPLAY_ONLY=1, RECOVERING=2, ACTIVE=3, BLOCKED=4, REBUILDING=5, LATEST_ACTIVE=6 });
vocabulary!(rejection_reason {
    NOT_ATTACHED=0, CHECKPOINTS_DISABLED=1, RESTORE_NOT_VERIFIED=2, OWNER_NOT_RUNNING=3,
    IDENTITY_MISMATCH=4, BEYOND_APPLIED=5, OFFSET_REGRESSED=6, OFFSET_CONFLICT=7,
    WRONG_RECOVERY_MODE=8, PROJECTOR_IDENTITY_MISMATCH=9, NOT_REBUILDING=10,
    REBUILD_NOT_COMPLETE=11, REBUILD_MISMATCH=12,
});
/// Recovery observations and requested modes are separate wire domains.
pub mod recovery_tag {
    pub const HISTORY_FROM_GENESIS: u32 = 0;
    pub const HISTORY_FROM_OFFSET: u32 = 1;
    pub const LATEST_STATE: u32 = 2;
}
pub mod recovery_mode {
    pub const CONTINUOUS_HISTORY: u32 = 1;
    pub const LATEST_STATE_ONLY: u32 = 2;
}
vocabulary!(query_key_tag { UNIQUE_KEY=0, SYS_ID=1 });
vocabulary!(query_reply_tag { PENDING=0, READY=1, REJECTED=2, DISCONNECTED=3 });
vocabulary!(query_rejection { UNKNOWN_KIND=1, UNKNOWN_UNIQUE_KEY=2, REPLY_TOO_LARGE=3, ENGINE_UNAVAILABLE=4 });
