//! Checks commit subjects and pull request titles against the format
//! `<gitmoji> <type>(<optional scope>): <summary>`.

/// Conventional Commits types accepted in a subject.
const TYPES: &[&str] = &[
    "feat", "fix", "docs", "style", "refactor", "perf", "test", "build", "ci", "chore", "revert",
];

const TYPE_PROBLEM: &str = "the type must be one of feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert";

/// The Git 50/72 rule: a subject never exceeds 72 characters.
const MAX_LENGTH: usize = 72;

/// Returns why `subject` breaks the commit format, or `None` when it follows it.
pub fn problem(subject: &str) -> Option<&'static str> {
    if subject.chars().count() > MAX_LENGTH {
        return Some("the subject must not exceed 72 characters");
    }
    let Some((_, rest)) = subject
        .split_once(' ')
        .filter(|(gitmoji, _)| is_gitmoji(gitmoji))
    else {
        return Some("the subject must start with a gitmoji followed by a space");
    };
    let Some((header, summary)) = rest.split_once(':') else {
        return Some(TYPE_PROBLEM);
    };
    let header = header.strip_suffix('!').unwrap_or(header);
    let commit_type = match header.split_once('(') {
        Some((commit_type, scope)) => {
            let scope = scope.strip_suffix(')').unwrap_or_default();
            if scope.is_empty() || scope.contains(|c: char| c.is_whitespace() || "()".contains(c)) {
                return Some("the scope must be a non-empty word in parentheses");
            }
            commit_type
        }
        None => header,
    };
    if !TYPES.contains(&commit_type) {
        return Some(TYPE_PROBLEM);
    }
    match summary.strip_prefix(' ') {
        Some(summary) if !summary.trim().is_empty() => {
            if summary.starts_with(char::is_uppercase) {
                Some("the summary must start with a lower case word")
            } else {
                summary
                    .ends_with('.')
                    .then_some("the summary must not end with a period")
            }
        }
        _ => Some("the type must be followed by \": \" and a summary"),
    }
}

/// Whether `token` is made of emoji characters only, as every gitmoji is.
// ponytail: emoji blocks rather than the gitmoji list, which changes over
// time; check against the list if other symbols start slipping through.
fn is_gitmoji(token: &str) -> bool {
    let is_symbol = |c: char| matches!(c, '\u{2190}'..='\u{2BFF}' | '\u{1F000}'..='\u{1FAFF}');
    let is_modifier = |c: char| matches!(c, '\u{FE0F}' | '\u{200D}' | '\u{20E3}');
    token.starts_with(is_symbol) && token.chars().all(|c| is_symbol(c) || is_modifier(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_subject_with_gitmoji_type_and_summary_passes() {
        assert_eq!(problem("👷 ci: run cargo xtask ci on x86_64 Linux"), None);
    }

    #[test]
    fn a_subject_with_a_scope_passes() {
        assert_eq!(problem("✨ feat(xtask): add the boundaries command"), None);
    }

    #[test]
    fn a_multi_codepoint_gitmoji_passes() {
        assert_eq!(problem("♻️ refactor(sim): split the tick loop"), None);
    }

    #[test]
    fn a_breaking_change_marker_passes() {
        assert_eq!(problem("💥 feat(protocol)!: rename the tick message"), None);
    }

    #[test]
    fn a_subject_without_gitmoji_is_reported() {
        assert_eq!(
            problem("feat(xtask): add the boundaries command"),
            Some("the subject must start with a gitmoji followed by a space")
        );
    }

    #[test]
    fn a_gitmoji_shortcode_is_reported() {
        assert_eq!(
            problem(":sparkles: feat: add the boundaries command"),
            Some("the subject must start with a gitmoji followed by a space")
        );
    }

    #[test]
    fn a_letter_in_place_of_the_gitmoji_is_reported() {
        assert_eq!(
            problem("é feat: add the boundaries command"),
            Some("the subject must start with a gitmoji followed by a space")
        );
    }

    #[test]
    fn a_zero_width_joiner_gitmoji_passes() {
        assert_eq!(problem("🧑‍💻 chore: improve the developer experience"), None);
    }

    #[test]
    fn an_upper_case_summary_is_reported() {
        assert_eq!(
            problem("✨ feat: Add the boundaries command"),
            Some("the summary must start with a lower case word")
        );
    }

    #[test]
    fn an_unknown_type_is_reported() {
        assert_eq!(
            problem("✨ feature: add the boundaries command"),
            Some(
                "the type must be one of feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert"
            )
        );
    }

    #[test]
    fn an_empty_scope_is_reported() {
        assert_eq!(
            problem("✨ feat(): add the boundaries command"),
            Some("the scope must be a non-empty word in parentheses")
        );
    }

    #[test]
    fn a_missing_colon_is_reported() {
        assert_eq!(
            problem("✨ feat add the boundaries command"),
            Some(
                "the type must be one of feat, fix, docs, style, refactor, perf, test, build, ci, chore, revert"
            )
        );
    }

    #[test]
    fn an_empty_summary_is_reported() {
        assert_eq!(
            problem("✨ feat: "),
            Some("the type must be followed by \": \" and a summary")
        );
    }

    #[test]
    fn a_trailing_period_is_reported() {
        assert_eq!(
            problem("✨ feat: add the boundaries command."),
            Some("the summary must not end with a period")
        );
    }

    #[test]
    fn a_subject_of_exactly_72_characters_passes() {
        let subject = format!("✨ feat: {}", "a".repeat(64));
        assert_eq!(subject.chars().count(), 72);

        assert_eq!(problem(&subject), None);
    }

    #[test]
    fn a_subject_over_72_characters_is_reported() {
        let subject = format!("✨ feat: {}", "a".repeat(65));
        assert_eq!(subject.chars().count(), 73);

        assert_eq!(
            problem(&subject),
            Some("the subject must not exceed 72 characters")
        );
    }
}
