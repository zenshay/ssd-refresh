//! Text formatting for saved summaries.
/// Like `para`, but returns the lines (each ending in '\n') instead of printing them.
/// The first line is indented by `first`, the others by `indent`.
pub fn paragraph(first: usize, indent: usize, text: &str) -> String {
    let w: usize = 100;
    let mut out = String::new();
    let mut line = String::new();
    let mut pad = first;
    for word in text.split_whitespace() {
        if !line.is_empty()
            && line.chars().count() + 1 + word.chars().count() > w.saturating_sub(pad).max(20)
        {
            out += &format!("{}{line}\n", " ".repeat(pad));
            line.clear();
            pad = indent;
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        out += &format!("{}{line}\n", " ".repeat(pad));
    }
    out
}
