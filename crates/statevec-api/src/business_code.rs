/// A deterministic rejection reason chosen by the business runtime.
/// Meaning belongs to that runtime's execution revision, not to the engine.
/// Zero is success; 60000 through 65535 are reserved for framework codes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BusinessRejectCode(u16);

impl BusinessRejectCode {
    /// Compatibility reason for runtimes that do not distinguish rejections.
    pub const UNSPECIFIED: Self = Self(1);

    pub const fn new(code: u16) -> Option<Self> {
        if code > 0 && code < 60000 { Some(Self(code)) } else { None }
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RejectedErrorCode;

    #[test]
    fn business_code_range_and_status_projection_are_exact() {
        for raw in 0..=u16::MAX {
            let code = BusinessRejectCode::new(raw);
            assert_eq!(code.is_some(), (1..60000).contains(&raw), "code {raw}");
            if let Some(code) = code {
                assert_eq!(code.get(), raw);
                let status = RejectedErrorCode::Business(code);
                assert_eq!(status.to_u16(), raw);
                assert_eq!(RejectedErrorCode::from_u16(raw), Some(status));
            }
        }
        assert_eq!(RejectedErrorCode::from_u16(60003), Some(RejectedErrorCode::RuntimePanic));
        assert_eq!(RejectedErrorCode::RuntimePanic.to_u16(), 60003);
        assert_eq!(RejectedErrorCode::from_u16(0), None);
        assert_eq!(RejectedErrorCode::from_u16(60000), None);
        assert_eq!(RejectedErrorCode::from_u16(u16::MAX), None);
    }
}
