use crate::{batch::Row, i18n::I18n};
use unicode_width::UnicodeWidthStr;

const HEADER_KEYS: [&str; 5] = [
    "report.header.file",
    "report.header.page",
    "report.header.correction",
    "report.header.counts",
    "report.header.status",
];
const RIGHT_ALIGNED: [bool; 5] = [false, true, true, true, false];

/// Format rows with the default Chinese headers and status text.
pub fn format_table(rows: &[Row]) -> String {
    format_table_localized(rows, &I18n::default())
}

/// Format rows with headers and status text translated by `i18n`.
///
/// Columns are still aligned by Unicode display width, so translated headers
/// keep the same layout guarantees as the default Chinese table.
pub fn format_table_localized(rows: &[Row], i18n: &I18n) -> String {
    let headers = HEADER_KEYS.map(|key| i18n.translate(key));
    format_table_with(rows, &headers, |row| row.status.render(i18n))
}

fn format_table_with(
    rows: &[Row],
    headers: &[String; 5],
    status_text: impl Fn(&Row) -> String,
) -> String {
    let cells: Vec<[String; 5]> = rows
        .iter()
        .map(|row| row_cells(row, &status_text(row)))
        .collect();
    let mut widths = headers.each_ref().map(|header| header.width());
    for row in &cells {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.width());
        }
    }
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(format_line(headers, &widths));
    lines.extend(cells.iter().map(|row| format_line(row, &widths)));
    lines.join("\n")
}

fn row_cells(row: &Row, status: &str) -> [String; 5] {
    let correction = row
        .correction
        .map_or("—".to_string(), |c| format!("{c:+.3}"));
    let counts = match (row.fragment_count, row.merged_count) {
        (Some(fragments), Some(merged)) => format!("{fragments} / {merged} / {}", row.count),
        _ => "— / — / —".to_string(),
    };
    [
        row.file.clone(),
        row.page.map_or("—".to_string(), |page| page.to_string()),
        correction,
        counts,
        status.to_string(),
    ]
}

fn format_line(cells: &[String; 5], widths: &[usize; 5]) -> String {
    let mut line = String::new();
    for (index, cell) in cells.iter().enumerate() {
        if index > 0 {
            line.push_str("  ");
        }
        let padding = " ".repeat(widths[index].saturating_sub(cell.width()));
        if RIGHT_ALIGNED[index] {
            line.push_str(&padding);
            line.push_str(cell);
        } else {
            line.push_str(cell);
            line.push_str(&padding);
        }
    }
    line.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        batch::RowReason,
        detection::{DetectionReason, Status},
    };
    use unicode_width::UnicodeWidthStr;

    fn row(
        file: &str,
        page: Option<usize>,
        correction: Option<f64>,
        counts: (Option<usize>, Option<usize>, usize),
        status: Status,
    ) -> Row {
        Row {
            file: file.into(),
            page,
            angle: correction.map(|c| -c),
            correction,
            status,
            count: counts.2,
            spread: None,
            reason: RowReason::Detection(DetectionReason::Consistent),
            preview: String::new(),
            fragment_count: counts.0,
            merged_count: counts.1,
        }
    }

    fn token_spans(line: &str) -> Vec<(usize, usize)> {
        let mut spans = Vec::new();
        let mut position = 0;
        for token in line
            .split("  ")
            .filter(|token| !token.is_empty())
            .map(str::trim_start)
        {
            let offset = line[position..].find(token).unwrap() + position;
            spans.push((line[..offset].width(), line[..offset + token.len()].width()));
            position = offset + token.len();
        }
        spans
    }

    fn sample_rows() -> [Row; 3] {
        [
            row(
                "a.png",
                Some(1),
                Some(1.0),
                (Some(1), Some(2), 3),
                Status::Trusted,
            ),
            row(
                "长名称.tif",
                Some(10),
                None,
                (Some(0), Some(0), 0),
                Status::NeedsReview,
            ),
            row(
                "broken.png",
                None,
                None,
                (None, None, 0),
                Status::ReadFailed,
            ),
        ]
    }

    fn assert_aligned(table: &str) {
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 4);
        let header = token_spans(lines[0]);
        assert_eq!(header.len(), 5);
        for line in &lines[1..] {
            let spans = token_spans(line);
            assert_eq!(spans.len(), 5, "{line}");
            for (index, (start, end)) in spans.iter().enumerate() {
                let (header_start, header_end) = header[index];
                if RIGHT_ALIGNED[index] {
                    assert_eq!(*end, header_end, "column {index}: {line}");
                } else {
                    assert_eq!(*start, header_start, "column {index}: {line}");
                }
            }
        }
    }

    #[test]
    fn aligns_columns_by_display_width() {
        let rows = sample_rows();
        let table = format_table(&rows);
        assert_aligned(&table);
        assert!(table.lines().next().unwrap().contains("转正角度（°）"));
        assert!(table.contains("可信"));
        assert!(table.contains("需人工确认"));
        assert!(table.contains("读取失败"));
    }

    #[test]
    fn localized_table_translates_headers_and_status() {
        let rows = sample_rows();
        let english = I18n::default().select("en-US").unwrap();
        let table = format_table_localized(&rows, &english);
        assert_aligned(&table);
        let header = table.lines().next().unwrap();
        for text in [
            "File",
            "Page",
            "Correction (°)",
            "Detected / merged / selected",
            "Status",
        ] {
            assert!(header.contains(text), "{header}");
        }
        assert!(table.contains("Trusted"));
        assert!(table.contains("Needs review"));
        assert!(table.contains("Read failed"));
        assert!(!table.contains("可信"));

        let chinese = I18n::default();
        assert_eq!(format_table_localized(&rows, &chinese), format_table(&rows));
    }
}
