//! Limits from the protocol's Bounds table, plus the clipping helpers that apply them.

pub const RPC_BODY: usize = 1024 * 1024;
pub const CELL_CODE: usize = 256 * 1024;
pub const CELL_STREAM: usize = 64 * 1024;
pub const CELL_RESULT: usize = 16 * 1024;
pub const TRACEBACK: usize = 16 * 1024;
pub const EVENT_TEXT: usize = 64 * 1024;
pub const QUEUED_EVENTS: usize = 256;
pub const CHILDREN_PER_PARENT: usize = 16;
pub const REGISTERED_TARGETS: usize = 1024;
pub const SSE_FRAME: usize = 1024 * 1024;
pub const TASK: usize = 32 * 1024;
pub const MESSAGE: usize = 32 * 1024;
pub const SKILL_DESCRIPTION: usize = 1024;
pub const TARGET_ID: usize = 256;
pub const MAX_TIMEOUT_SEC: u64 = 3600;

const HEAD_MARKER: &str = "[… truncated …]\n";
const TAIL_MARKER: &str = "\n[… truncated …]";

/// Keeps the last bytes of `text` within `limit`, prefixed by a marker. Returns true if clipped.
pub fn clip_tail(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }

    let keep = limit.saturating_sub(HEAD_MARKER.len());
    let mut start = text.len().saturating_sub(keep);

    while !text.is_char_boundary(start) {
        start = start.saturating_add(1);
    }

    let tail = text.split_off(start);
    *text = format!("{HEAD_MARKER}{tail}");

    true
}

/// Keeps the first bytes of `text` within `limit`, suffixed by a marker. Returns true if clipped.
pub fn clip_head(text: &mut String, limit: usize) -> bool {
    if text.len() <= limit {
        return false;
    }

    let mut end = limit.saturating_sub(TAIL_MARKER.len());

    while !text.is_char_boundary(end) {
        end = end.saturating_sub(1);
    }

    text.truncate(end);
    text.push_str(TAIL_MARKER);

    true
}

#[cfg(test)]
mod tests {
    use super::{clip_head, clip_tail};

    #[test]
    fn clip_tail_keeps_the_end_within_the_limit() {
        let mut text = "é".repeat(100);

        assert!(clip_tail(&mut text, 64));
        assert!(text.len() <= 64);
        assert!(text.ends_with('é'));
        assert!(text.starts_with("[…"));
    }

    #[test]
    fn clip_head_keeps_the_start_within_the_limit() {
        let mut text = "é".repeat(100);

        assert!(clip_head(&mut text, 64));
        assert!(text.len() <= 64);
        assert!(text.starts_with('é'));
    }

    #[test]
    fn short_text_is_untouched() {
        let mut text = String::from("hi");

        assert!(!clip_tail(&mut text, 64));
        assert!(!clip_head(&mut text, 64));
        assert_eq!(text, "hi");
    }
}
