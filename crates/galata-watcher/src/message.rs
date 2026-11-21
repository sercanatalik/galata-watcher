//! A message, written once and rendered twice: as plain text, which is what
//! the alert history keeps and galata-desk shows, and as the HTML subset
//! Telegram accepts, which is what is sent.
//!
//! The plain rendering is the message's wording. The HTML rendering adds
//! only emphasis, an icon and aligned tables, so the two never say
//! different things. Every piece of text is escaped for HTML here, so a
//! level's note or a market name can never break the markup.

/// One stretch of a line.
#[derive(Debug, Clone, PartialEq)]
pub enum Span {
    Text(String),
    Bold(String),
    Italic(String),
    /// A figure: monospaced in Telegram.
    Code(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone, PartialEq)]
enum Block {
    Line(Vec<Span>),
    /// A section's name: `Open positions:` in plain text, bold in HTML.
    Heading(String),
    /// Rows of cells. Plain text joins a row's non-empty cells with one
    /// space; HTML pads them into columns inside `<pre>`.
    Table { rows: Vec<Vec<String>>, align: Vec<Align> },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Message {
    /// Shown before the first line in HTML only.
    icon: Option<&'static str>,
    blocks: Vec<Block>,
}

impl Message {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn icon(mut self, icon: &'static str) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn line(mut self, spans: Vec<Span>) -> Self {
        self.blocks.push(Block::Line(spans));
        self
    }

    pub fn heading(mut self, name: impl Into<String>) -> Self {
        self.blocks.push(Block::Heading(name.into()));
        self
    }

    /// `align` names each column's alignment; a column past its end is left-aligned.
    pub fn table(mut self, rows: Vec<Vec<String>>, align: &[Align]) -> Self {
        if !rows.is_empty() {
            self.blocks.push(Block::Table { rows, align: align.to_vec() });
        }
        self
    }

    pub fn plain(&self) -> String {
        let mut lines = Vec::new();
        for block in &self.blocks {
            match block {
                Block::Line(spans) => lines.push(
                    spans
                        .iter()
                        .map(|s| match s {
                            Span::Text(t) | Span::Bold(t) | Span::Italic(t) | Span::Code(t) => t.as_str(),
                        })
                        .collect(),
                ),
                Block::Heading(name) => lines.push(format!("{name}:")),
                Block::Table { rows, .. } => {
                    for row in rows {
                        let cells: Vec<&str> = row.iter().map(String::as_str).filter(|c| !c.is_empty()).collect();
                        lines.push(cells.join(" "));
                    }
                }
            }
        }
        lines.join("\n")
    }

    pub fn html(&self) -> String {
        let mut lines: Vec<String> = Vec::new();
        for (i, block) in self.blocks.iter().enumerate() {
            match block {
                Block::Line(spans) => {
                    let mut line: String = spans
                        .iter()
                        .map(|s| match s {
                            Span::Text(t) => escape(t),
                            Span::Bold(t) => format!("<b>{}</b>", escape(t)),
                            Span::Italic(t) => format!("<i>{}</i>", escape(t)),
                            Span::Code(t) => format!("<code>{}</code>", escape(t)),
                        })
                        .collect();
                    if i == 0
                        && let Some(icon) = self.icon
                    {
                        line = format!("{icon} {line}");
                    }
                    lines.push(line);
                }
                Block::Heading(name) => lines.push(format!("<b>{}</b>", escape(name))),
                Block::Table { rows, align } => lines.push(format!("<pre>{}</pre>", escape(&columns(rows, align)))),
            }
        }
        lines.join("\n")
    }
}

/// Rows padded into columns, two spaces apart. Empty cells keep their column.
fn columns(rows: &[Vec<String>], align: &[Align]) -> String {
    let n = rows.iter().map(Vec::len).max().unwrap_or(0);
    let width = |c: usize| rows.iter().filter_map(|r| r.get(c)).map(|s| s.chars().count()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..n).map(width).collect();
    rows.iter()
        .map(|row| {
            let cells: Vec<String> = (0..n)
                .map(|c| {
                    let cell = row.get(c).map_or("", String::as_str);
                    let pad = widths[c] - cell.chars().count();
                    match align.get(c).copied().unwrap_or(Align::Left) {
                        Align::Left => format!("{cell}{}", " ".repeat(pad)),
                        Align::Right => format!("{}{cell}", " ".repeat(pad)),
                    }
                })
                .collect();
            cells.join("  ").trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The three characters Telegram's HTML needs escaped.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

pub fn text(s: impl Into<String>) -> Span {
    Span::Text(s.into())
}
pub fn bold(s: impl Into<String>) -> Span {
    Span::Bold(s.into())
}
pub fn italic(s: impl Into<String>) -> Span {
    Span::Italic(s.into())
}
pub fn code(s: impl Into<String>) -> Span {
    Span::Code(s.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_drops_the_emphasis_and_html_keeps_it() {
        let m = Message::new().icon("🔴").line(vec![bold("BTC stop-loss filled"), text(": sold "), code("0.5")]);
        assert_eq!(m.plain(), "BTC stop-loss filled: sold 0.5");
        assert_eq!(m.html(), "🔴 <b>BTC stop-loss filled</b>: sold <code>0.5</code>");
    }

    #[test]
    fn text_is_escaped_in_html_only() {
        let m = Message::new().line(vec![text("support <58k> & "), bold("a<b")]);
        assert_eq!(m.plain(), "support <58k> & a<b");
        assert_eq!(m.html(), "support &lt;58k&gt; &amp; <b>a&lt;b</b>");
    }

    #[test]
    fn a_table_is_words_in_plain_text_and_columns_in_html() {
        let rows = vec![
            vec!["24h".to_string(), "+$1,235".to_string()],
            vec!["All-time".to_string(), "+$18,870".to_string()],
            vec!["Closed / other".to_string(), String::new(), "-$5".to_string()],
        ];
        let m = Message::new().heading("Windows").table(rows, &[Align::Left, Align::Right, Align::Right]);
        assert_eq!(m.plain(), "Windows:\n24h +$1,235\nAll-time +$18,870\nClosed / other -$5");
        assert_eq!(
            m.html(),
            "<b>Windows</b>\n<pre>24h              +$1,235\nAll-time        +$18,870\nClosed / other            -$5</pre>"
        );
    }

    #[test]
    fn an_empty_table_adds_nothing() {
        assert_eq!(Message::new().line(vec![text("a")]).table(vec![], &[]).plain(), "a");
    }
}
