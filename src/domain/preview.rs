use unicode_segmentation::UnicodeSegmentation;

pub const MENU_PREVIEW_GRAPHEMES: usize = 15;

pub fn menu_preview(text: &str) -> String {
    let compact = text
        .chars()
        .filter(|character| !character.is_whitespace())
        .collect::<String>();
    let graphemes = compact.graphemes(true).collect::<Vec<_>>();

    if graphemes.len() <= MENU_PREVIEW_GRAPHEMES {
        return compact;
    }

    let mut preview = graphemes
        .into_iter()
        .take(MENU_PREVIEW_GRAPHEMES)
        .collect::<String>();
    preview.push('…');
    preview
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_all_whitespace_before_truncating() {
        assert_eq!(menu_preview("  hello\n\tworld  "), "helloworld");
    }

    #[test]
    fn truncates_after_whitespace_removal() {
        assert_eq!(menu_preview("12345 67890 abcdef"), "1234567890abcde…");
    }

    #[test]
    fn counts_unicode_graphemes() {
        assert_eq!(menu_preview("👨‍👩‍👧‍👦123456789012345"), "👨‍👩‍👧‍👦12345678901234…");
    }
}
