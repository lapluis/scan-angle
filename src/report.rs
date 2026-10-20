use crate::batch::Row;
use unicode_width::UnicodeWidthStr;

const HEADERS: [&str; 5] = ["文件", "页", "转正角度（°）", "检测 / 合并 / 入选", "状态"];
const RIGHT_ALIGNED: [bool; 5] = [false, true, true, true, false];

pub fn format_table(rows: &[Row]) -> String {
    let cells: Vec<[String; 5]> = rows.iter().map(row_cells).collect();
    let mut widths = HEADERS.map(|header| header.width());
    for row in &cells {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.width());
        }
    }
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(format_line(&HEADERS.map(str::to_string), &widths));
    lines.extend(cells.iter().map(|row| format_line(row, &widths)));
    lines.join("\n")
}

fn row_cells(row: &Row) -> [String; 5] {
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
        row.status.clone(),
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
    use unicode_width::UnicodeWidthStr;

    fn row(
        file: &str,
        page: Option<usize>,
        correction: Option<f64>,
        counts: (Option<usize>, Option<usize>, usize),
        status: &str,
    ) -> Row {
        Row {
            file: file.into(),
            page,
            angle: correction.map(|c| -c),
            correction,
            status: status.into(),
            count: counts.2,
            spread: None,
            reason: String::new(),
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

    #[test]
    fn aligns_columns_by_display_width() {
        let rows = [
            row("a.png", Some(1), Some(1.0), (Some(1), Some(2), 3), "可信"),
            row(
                "长名称.tif",
                Some(10),
                None,
                (Some(0), Some(0), 0),
                "需人工确认",
            ),
            row("broken.png", None, None, (None, None, 0), "读取失败"),
        ];
        let table = format_table(&rows);
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
}
