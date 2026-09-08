use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
/// A bare line number is ambiguous in a diff: deletions use the old file's
/// numbering while additions and context use the new file's.
pub(crate) enum Side {
    Old,
    New,
}

impl Side {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Old => "old",
            Self::New => "new",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum CommentState {
    Open,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct Comment {
    pub(crate) path: String,
    pub(crate) line: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) end_line: Option<u32>,
    pub(crate) side: Side,
    pub(crate) body: String,
    pub(crate) state: CommentState,
}

impl Comment {
    pub(crate) fn open(path: String, line: u32, side: Side, body: String) -> Self {
        Self::range(path, line, line, side, body)
    }

    pub(crate) fn range(path: String, start: u32, end: u32, side: Side, body: String) -> Self {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        Self {
            path,
            line: start,
            end_line: (end > start).then_some(end),
            side,
            body,
            state: CommentState::Open,
        }
    }

    pub(crate) fn start_line(&self) -> u32 {
        self.line
    }

    pub(crate) fn end_line(&self) -> u32 {
        self.end_line.unwrap_or(self.line)
    }

    pub(crate) fn covers(&self, path: &str, line: u32, side: Side) -> bool {
        self.path == path
            && self.side == side
            && line >= self.start_line()
            && line <= self.end_line()
    }

    pub(crate) fn location(&self) -> String {
        if self.start_line() == self.end_line() {
            format!("{}:{}", self.path, self.start_line())
        } else {
            format!("{}:{}-{}", self.path, self.start_line(), self.end_line())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Comment, CommentState, Side};

    #[test]
    fn comments_round_trip_through_json_without_losing_review_state() {
        let comment = Comment {
            path: "src/main.rs".to_owned(),
            line: 12,
            end_line: Some(18),
            side: Side::New,
            body: "Handle the error at this boundary.".to_owned(),
            state: CommentState::Open,
        };

        let json =
            serde_json::to_string(&comment).expect("the comment model must serialize to JSON");
        let decoded: Comment =
            serde_json::from_str(&json).expect("serialized comments must deserialize");

        assert_eq!(
            decoded, comment,
            "JSON round trips must preserve every comment field"
        );
        assert!(
            json.contains("\"end_line\":18"),
            "range comments must persist the last covered line: {json}"
        );
    }

    #[test]
    fn single_line_comments_omit_end_line_and_accept_legacy_json() {
        let comment = Comment::open(
            "src/main.rs".to_owned(),
            12,
            Side::New,
            "Handle the error at this boundary.".to_owned(),
        );
        let json =
            serde_json::to_string(&comment).expect("the comment model must serialize to JSON");
        assert!(
            !json.contains("end_line"),
            "single-line comments must stay backward-compatible: {json}"
        );

        let decoded: Comment = serde_json::from_str(
            r#"{"path":"src/main.rs","line":12,"side":"new","body":"Handle the error at this boundary.","state":"open"}"#,
        )
        .expect("revisions saved before range comments must still load");
        assert_eq!(decoded, comment);
        assert_eq!(decoded.end_line(), 12);
        assert!(decoded.covers("src/main.rs", 12, Side::New));
        assert!(!decoded.covers("src/main.rs", 13, Side::New));
    }

    #[test]
    fn range_comments_cover_every_line_in_the_span() {
        let comment = Comment::range(
            "src/lib.rs".to_owned(),
            18,
            12,
            Side::Old,
            "this whole block".to_owned(),
        );
        assert_eq!(comment.line, 12);
        assert_eq!(comment.end_line, Some(18));
        assert_eq!(comment.location(), "src/lib.rs:12-18");
        assert!(comment.covers("src/lib.rs", 12, Side::Old));
        assert!(comment.covers("src/lib.rs", 15, Side::Old));
        assert!(comment.covers("src/lib.rs", 18, Side::Old));
        assert!(!comment.covers("src/lib.rs", 11, Side::Old));
        assert!(!comment.covers("src/lib.rs", 15, Side::New));
    }
}
