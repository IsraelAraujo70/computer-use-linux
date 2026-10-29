//! Text recognition over screenshots through the `tesseract` CLI, for surfaces
//! that expose no accessibility tree (canvas, games, Electron without
//! `--force-renderer-accessibility`).

use crate::command_runner;
use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::Serialize;
use std::collections::BTreeMap;
use std::time::Duration;
use tokio::process::Command;

const OCR_TIMEOUT: Duration = Duration::from_secs(30);

/// One recognized text line, in the pixel space of the image it came from.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct OcrLine {
    pub text: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    /// Mean word confidence, 0 to 100.
    pub confidence: f32,
}

impl OcrLine {
    /// `@x,y wxh text`, matching the outline's bounds notation.
    pub fn render(&self) -> String {
        format!(
            "@{},{} {}x{} {}",
            self.x, self.y, self.width, self.height, self.text
        )
    }

    pub fn offset(mut self, dx: i32, dy: i32) -> Self {
        self.x += dx;
        self.y += dy;
        self
    }
}

/// Runs tesseract on PNG bytes. `lang` defaults to every installed language
/// among Portuguese and English.
pub async fn recognize(
    png: Vec<u8>,
    lang: Option<&str>,
    min_confidence: f32,
) -> Result<Vec<OcrLine>> {
    let lang = match lang.map(str::trim).filter(|lang| !lang.is_empty()) {
        Some(lang) => lang.to_string(),
        None => default_languages().await,
    };
    let mut command = Command::new("tesseract");
    command.args(["stdin", "stdout", "--psm", "3", "-l", &lang, "tsv"]);
    let output = command_runner::output_with_stdin(command, "run tesseract", OCR_TIMEOUT, png)
        .await
        .context("tesseract is required for OCR; install tesseract-ocr")?;
    if !output.status.success() {
        bail!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(parse_tsv(
        &String::from_utf8_lossy(&output.stdout),
        min_confidence,
    ))
}

async fn default_languages() -> String {
    let mut command = Command::new("tesseract");
    command.arg("--list-langs");
    let installed = command_runner::output(command, "list tesseract languages")
        .await
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default();
    let langs: Vec<&str> = ["por", "eng"]
        .into_iter()
        .filter(|lang| installed.lines().any(|line| line.trim() == *lang))
        .collect();
    if langs.is_empty() {
        "eng".to_string()
    } else {
        langs.join("+")
    }
}

/// Groups tesseract's word rows (level 5) into lines keyed by
/// block/paragraph/line, dropping words below `min_confidence`.
pub fn parse_tsv(tsv: &str, min_confidence: f32) -> Vec<OcrLine> {
    struct Acc {
        words: Vec<String>,
        left: i32,
        top: i32,
        right: i32,
        bottom: i32,
        confidence_sum: f32,
    }
    let mut lines: BTreeMap<(u32, u32, u32, u32), Acc> = BTreeMap::new();
    for row in tsv.lines().skip(1) {
        let cols: Vec<&str> = row.split('\t').collect();
        if cols.len() < 12 || cols[0] != "5" {
            continue;
        }
        let num = |i: usize| cols[i].trim().parse::<i32>().unwrap_or(0);
        let confidence = cols[10].trim().parse::<f32>().unwrap_or(-1.0);
        let text = cols[11].trim();
        if text.is_empty() || confidence < min_confidence {
            continue;
        }
        let key = (num(1) as u32, num(2) as u32, num(3) as u32, num(4) as u32);
        let (left, top, width, height) = (num(6), num(7), num(8), num(9));
        let acc = lines.entry(key).or_insert(Acc {
            words: Vec::new(),
            left,
            top,
            right: left + width,
            bottom: top + height,
            confidence_sum: 0.0,
        });
        acc.words.push(text.to_string());
        acc.left = acc.left.min(left);
        acc.top = acc.top.min(top);
        acc.right = acc.right.max(left + width);
        acc.bottom = acc.bottom.max(top + height);
        acc.confidence_sum += confidence;
    }
    lines
        .into_values()
        .map(|acc| OcrLine {
            confidence: (acc.confidence_sum / acc.words.len() as f32).round(),
            text: acc.words.join(" "),
            x: acc.left,
            y: acc.top,
            width: acc.right - acc.left,
            height: acc.bottom - acc.top,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext";

    #[test]
    fn words_group_into_lines_with_union_bounds() {
        let tsv = format!(
            "{HEADER}\n\
             4\t1\t1\t1\t1\t0\t10\t20\t200\t18\t-1\t\n\
             5\t1\t1\t1\t1\t1\t10\t20\t60\t18\t96\tAdd\n\
             5\t1\t1\t1\t1\t2\t80\t22\t70\t16\t90\tdevice\n\
             5\t1\t1\t1\t2\t1\t10\t50\t40\t18\t20\tnoise\n\
             5\t1\t2\t1\t1\t1\t300\t20\t50\t18\t88\tExport\n"
        );

        let lines = parse_tsv(&tsv, 50.0);

        assert_eq!(
            lines,
            vec![
                OcrLine {
                    text: "Add device".into(),
                    x: 10,
                    y: 20,
                    width: 140,
                    height: 18,
                    confidence: 93.0
                },
                OcrLine {
                    text: "Export".into(),
                    x: 300,
                    y: 20,
                    width: 50,
                    height: 18,
                    confidence: 88.0
                },
            ]
        );
        assert_eq!(lines[0].render(), "@10,20 140x18 Add device");
    }
}
