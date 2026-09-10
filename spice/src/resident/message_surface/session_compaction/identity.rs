use super::*;

const SESSION_IDENTITY_HEX_DIGITS: u32 = 15;

/// Namespace preserved-session loci away from tape member ordinals while
/// retaining a deterministic identity across resident and browser restarts.
pub(in crate::resident::message_surface) fn session_member(thread: &str) -> u64 {
    let mut value = 0_u64;
    let mut digits = 0_u32;
    for byte in thread.bytes().filter(u8::is_ascii_hexdigit) {
        let nibble = match byte {
            b'0'..=b'9' => u64::from(byte - b'0'),
            b'a'..=b'f' => u64::from(byte - b'a' + 10),
            b'A'..=b'F' => u64::from(byte - b'A' + 10),
            _ => unreachable!("the filter admits only hexadecimal digits"),
        };
        value = (value << 4) | nibble;
        digits += 1;
        if digits == SESSION_IDENTITY_HEX_DIGITS {
            break;
        }
    }
    SESSION_MESSAGE_MEMBER_NAMESPACE | value
}
