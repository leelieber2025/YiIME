use std::io::{self, BufRead, Write};
use yiime_core::{Candidate, Draft, MixedInput, translate_openai};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = MixedInput::bundled();
    let mut draft = Draft::default();
    println!(
        "中文草稿测试：输入拼音或五笔码查看候选；:n 选第 n 项；:back 删除；:left/:right 移光标；:translate 翻译；:commit 输出英文；:quit 退出"
    );
    let mut stdin = io::stdin().lock();
    let mut candidates: Vec<Candidate> = Vec::new();
    let mut raw = Vec::new();
    loop {
        raw.clear();
        if stdin.read_until(b'\n', &mut raw)? == 0 {
            break;
        }
        // Non-UTF-8 lines (e.g. piped into GBK) are treated as replacement characters without
        // interrupting subsequent input.
        let line = String::from_utf8_lossy(&raw);
        let command = line.trim();
        match command {
            ":quit" => break,
            ":back" => draft.backspace(),
            ":left" => draft.move_cursor(-1),
            ":right" => draft.move_cursor(1),
            ":translate" => match draft.translate_with(translate_openai) {
                Ok(()) => println!("译文预览：{}", draft.translation().unwrap_or_default()),
                Err(error) => eprintln!("翻译失败：{error}"),
            },
            ":commit" => match draft.take_english() {
                Some(english) => println!("提交英文：{english}"),
                None => eprintln!("请先翻译"),
            },
            _ if command.starts_with(':') => {
                if let Ok(index) = command[1..].parse::<usize>() {
                    if let Some(candidate) = index.checked_sub(1).and_then(|i| candidates.get(i)) {
                        draft.insert(&candidate.text);
                    } else {
                        eprintln!("候选序号无效");
                    }
                } else {
                    eprintln!("未知命令");
                }
            }
            "" => {}
            _ => {
                candidates = input.candidates(command, 9);
                for (index, candidate) in candidates.iter().enumerate() {
                    println!(
                        "{}. {} [{}]",
                        index + 1,
                        candidate.text,
                        if candidate.complete {
                            "完整码"
                        } else {
                            "前缀"
                        }
                    );
                }
            }
        }
        println!("草稿：{}  光标：{}", draft.text(), draft.cursor());
        io::stdout().flush()?;
    }
    Ok(())
}
