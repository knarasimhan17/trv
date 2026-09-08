use std::io::{self, Write};

use anyhow::{Context, Result};

pub(crate) fn deliver(formatted: &str) -> Result<()> {
    if formatted.is_empty() {
        return Ok(());
    }
    let mut output = io::stdout().lock();
    writeln!(output, "{formatted}").context("failed to write comments to stdout")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn bundled_skill_teaches_the_in_session_review_loop() {
        let skill = include_str!("../skills/trv/SKILL.md");
        assert!(
            skill.contains("name: trv\n"),
            "the skill name must match skills/trv/"
        );
        assert!(
            skill.contains("trv --agent"),
            "agents must run trv --agent after making code changes"
        );
        assert!(
            skill.contains("3600000"),
            "agents must wait long enough for a human review"
        );
        assert!(
            !skill.to_ascii_lowercase().contains("another terminal"),
            "the review must happen in the agent session"
        );
        assert!(
            !skill.contains("Terminal.app") && !skill.to_ascii_lowercase().contains("popup"),
            "the skill must not send the review to a separate window"
        );
    }
}
