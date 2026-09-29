//! Panic report formatting (issue #24 / spec §11.6).

use lush_ir::bytecode::Function;

pub const MAX_PANIC_MESSAGE_BYTES: usize = 4096;
pub const MAX_PANIC_FRAMES: usize = 64;

#[derive(Clone, Debug)]
pub struct FrameInfo {
    pub module: String,
    pub function: String,
    pub source_path: String,
    pub line: u32,
    pub col: u32,
}

pub fn format_panic(
    message: &str,
    site: &FrameInfo,
    frames: &[FrameInfo],
    had_tail_call: bool,
) -> String {
    let mut out = String::new();
    out.push_str("panic: ");
    out.push_str(&escape_ctrl(&truncate(message, MAX_PANIC_MESSAGE_BYTES)));
    out.push('\n');
    out.push_str(&format!(
        "at {}:{}:{}",
        escape_ctrl(&site.source_path),
        site.line,
        site.col
    ));
    out.push('\n');
    if frames.len() > MAX_PANIC_FRAMES {
        let keep = MAX_PANIC_FRAMES / 2;
        for fr in &frames[..keep] {
            push_frame(&mut out, fr);
        }
        out.push_str("  ...\n");
        for fr in &frames[frames.len() - keep..] {
            push_frame(&mut out, fr);
        }
    } else {
        for fr in frames {
            push_frame(&mut out, fr);
        }
    }
    if had_tail_call {
        out.push_str("(tail-called frames are not shown)\n");
    }
    out
}

fn push_frame(out: &mut String, fr: &FrameInfo) {
    out.push_str(&format!(
        "  in {}.{} ({}:{}:{})\n",
        escape_ctrl(&fr.module),
        escape_ctrl(&fr.function),
        escape_ctrl(&fr.source_path),
        fr.line,
        fr.col
    ));
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}

pub fn escape_ctrl(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_control() {
            out.push_str(&format!("\\u{{{:x}}}", ch as u32));
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn frame_from_function(f: &Function, line: u32, col: u32) -> FrameInfo {
    FrameInfo {
        module: f.module.clone(),
        function: f.name.clone(),
        source_path: f.source_path.clone(),
        line,
        col,
    }
}
