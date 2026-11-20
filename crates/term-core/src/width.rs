//! Which characters occupy two terminal columns, so the grid can lay them out
//! the way a conventional terminal does. Ranges come from the Unicode East Asian Width
//! property, not from a font, so a glyph's width is decided before anything is
//! rendered.

/// Returns true when `c` should be painted across two columns.
///
/// The original's own font list (Roboto Mono, Fira Code, …) is monospaced, so
/// wide glyphs are CJK ideographs, CJK punctuation, full-width forms, and the
/// emoji blocks that terminals draw as 2-wide.
pub fn is_wide(c: char) -> bool {
    let u = c as u32;
    if u < 0x1100 {
        return false;
    }
    // Hangul syllables and jamo
    if (0x1100..=0x11FF).contains(&u)
        || (0x2E80..=0x2FFF).contains(&u)
        || (0x3000..=0x303F).contains(&u)
        || (0x3040..=0x33FF).contains(&u)
        || (0x3400..=0x4DBF).contains(&u)
        || (0x4E00..=0x9FFF).contains(&u)
        || (0xA000..=0xA4CF).contains(&u)
        || (0xA960..=0xA97F).contains(&u)
        || (0xAC00..=0xD7AF).contains(&u)
        || (0xF900..=0xFAFF).contains(&u)
        || (0xFE10..=0xFE1F).contains(&u)
        || (0xFE30..=0xFE4F).contains(&u)
        || (0xFF00..=0xFF60).contains(&u)
        || (0xFFE0..=0xFFE6).contains(&u)
        || (0x1F300..=0x1F64F).contains(&u)
        || (0x1F680..=0x1F6FF).contains(&u)
        || (0x1F900..=0x1F9FF).contains(&u)
        || (0x20000..=0x2FA1F).contains(&u)
        || (0x2F800..=0x2FA1F).contains(&u)
        || (0x30000..=0x3134F).contains(&u)
    {
        return true;
    }
    // Combining marks and zero-width things are not wide.
    if (0x0300..=0x036F).contains(&u)
        || (0x1AB0..=0x1AFF).contains(&u)
        || (0x1DC0..=0x1DFF).contains(&u)
        || (0x20D0..=0x20FF).contains(&u)
        || (0xFE20..=0xFE2F).contains(&u)
    {
        return false;
    }
    false
}