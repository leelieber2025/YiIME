//! `yiime query <code>…`: print candidates from the same core engine as the Windows input box.
//! `yiime translate <text>…`: free Google Translate with per-request timing.

use std::path::PathBuf;
use std::process::ExitCode;
use yiime_core::{
    ComposeOptions, Composer, Learned, MixedInput, PAGE_SIZE, PAGE_SIZE_RANGE, Shuangpin,
};

const USAGE: &str = "用法：yiime translate <中文>…（免费 Google 翻译，打印每次请求的计时）
      yiime query <编码>… [--page-size N] [--pages N] [--traditional] [--fuzzy] [--pinyin-first] [--all-chars] [--shuangpin xiaohe|microsoft|ziranma] [--words 文件 | --no-words] [--tsv]
      yiime import <词库文件> [--into user_words.txt]（导入搜狗/QQ 导出的 txt 或搜狗 .scel；不加 --into 时打印到标准输出）
  大词库默认查找顺序：--words、环境变量 YIIME_WORDS、程序旁的 InputTranslation-words.txt、
  上级目录中的 data/frost/InputTranslation-words.txt 或 share/yiime/InputTranslation-words.txt。
  --words 的路径不能以 - 开头（写成 ./-文件名）。
  默认与输入法一样只显示常用字（不出扩展区生僻字、低频简拼词）；--all-chars 显示全部。";

const WORDS_FILE: &str = "InputTranslation-words.txt";

fn find_words() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("YIIME_WORDS") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().skip(1).find_map(|dir| {
        [
            dir.join(WORDS_FILE),
            dir.join("data").join("frost").join(WORDS_FILE),
            dir.join("share").join("yiime").join(WORDS_FILE),
        ]
        .into_iter()
        .find(|path| path.is_file())
    })
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("translate") {
        return translate(&args[1..]);
    }
    if args.first().map(String::as_str) == Some("import") {
        return import(&args[1..]);
    }
    if args.first().map(String::as_str) != Some("query") {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let mut codes = Vec::new();
    let mut page_size = PAGE_SIZE;
    let mut pages = 1usize;
    let mut traditional = false;
    let mut fuzzy = false;
    let mut pinyin_first = false;
    let mut common_only = true;
    let mut shuangpin = None;
    let mut tsv = false;
    let mut words: Option<Option<PathBuf>> = None;
    let mut rest = args[1..].iter();
    let fail = |error: &str| {
        eprintln!("{error}\n{USAGE}");
        ExitCode::from(2)
    };
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--page-size" | "--pages" => {
                let Some(value) = rest.next().and_then(|value| value.parse::<usize>().ok()) else {
                    return fail(&format!("{arg} 需要一个数字"));
                };
                if arg == "--pages" {
                    pages = value.max(1);
                } else {
                    page_size = value.clamp(*PAGE_SIZE_RANGE.start(), *PAGE_SIZE_RANGE.end());
                }
            }
            "--traditional" => traditional = true,
            "--fuzzy" => fuzzy = true,
            "--pinyin-first" => pinyin_first = true,
            "--all-chars" => common_only = false,
            "--shuangpin" => {
                let Some(scheme) = rest.next().and_then(|id| Shuangpin::from_id(id)) else {
                    return fail("--shuangpin 需要 xiaohe、microsoft 或 ziranma");
                };
                shuangpin = Some(scheme);
            }
            "--tsv" => tsv = true,
            "--no-words" => words = Some(None),
            "--words" => {
                let Some(path) = option_value(rest.next()) else {
                    return fail("缺少 --words 的文件路径（missing path for --words）");
                };
                words = Some(Some(PathBuf::from(path)));
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            other if other.starts_with("--") => return fail(&format!("未知选项 {other}")),
            code => codes.push(code.to_owned()),
        }
    }
    if codes.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let mut input = MixedInput::bundled();
    let words = words.unwrap_or_else(find_words);
    if let Some(path) = &words {
        match std::fs::read_to_string(path) {
            Ok(text) => input = input.with_frequency_words(text),
            Err(error) => {
                eprintln!("读取大词库 {} 失败：{error}", path.display());
                return ExitCode::FAILURE;
            }
        }
    } else if !tsv {
        eprintln!("（未找到大词库，只用内置词库）");
    }
    let options = ComposeOptions {
        page_size,
        traditional,
        learn: false,
        fuzzy,
        pinyin_first,
        common_only,
        shuangpin,
    };
    let learned = Learned::default();
    let mut composer = Composer::default();
    for code in codes {
        composer.set_code(&input, code.to_lowercase(), &options, &learned);
        if !tsv {
            println!("{}  {}", composer.code, composer.page_label());
        }
        for page in 0..pages.min(composer.page_count()) {
            composer.page = page;
            for (index, candidate) in composer.page_items().iter().enumerate() {
                let overall = page * page_size + index;
                if tsv {
                    println!(
                        "{page_size}\t{}\t{overall}\t{}\t{:?}\t{}\t{}",
                        composer.code,
                        candidate.text,
                        candidate.source,
                        candidate.complete,
                        composer.hint(candidate)
                    );
                } else {
                    println!(
                        "  {}{}. {}{}  [{:?}{}{}]",
                        if pages > 1 {
                            format!("p{} ", page + 1)
                        } else {
                            String::new()
                        },
                        index + 1,
                        candidate.text,
                        composer.hint(candidate),
                        candidate.source,
                        if candidate.complete { "" } else { ", 前缀" },
                        if candidate.rest.is_empty() {
                            String::new()
                        } else {
                            format!(", 余 {}", candidate.rest)
                        }
                    );
                }
            }
        }
    }
    ExitCode::SUCCESS
}

/// `yiime import <file> [--into user_words.txt]`: converts another IME's exported user
/// dictionary into user words.
fn import(args: &[String]) -> ExitCode {
    let (file, into) = match args {
        [file] => (file, None),
        [file, flag, into] if flag == "--into" => (file, Some(into)),
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let words = match std::fs::read(file) {
        Ok(bytes) => match yiime_core::import_words(&bytes) {
            Ok(words) => words,
            Err(error) => {
                eprintln!("{file}：{error}");
                return ExitCode::FAILURE;
            }
        },
        Err(error) => {
            eprintln!("读取 {file} 失败：{error}");
            return ExitCode::FAILURE;
        }
    };
    let Some(into) = into else {
        for (word, code) in &words {
            println!("{word} {code}");
        }
        return ExitCode::SUCCESS;
    };
    let existing = std::fs::read_to_string(into).unwrap_or_default();
    let (contents, added) = yiime_core::merge_user_words(&existing, &words);
    if let Err(error) = std::fs::write(into, contents) {
        eprintln!("写入 {into} 失败：{error}");
        return ExitCode::FAILURE;
    }
    eprintln!("已导入 {added} 个新词（共读到 {} 个）", words.len());
    ExitCode::SUCCESS
}

/// Value of an option that takes a path: missing or another option (`-…`) is an error.
fn option_value(value: Option<&String>) -> Option<&str> {
    value
        .map(String::as_str)
        .filter(|value| !value.is_empty() && !value.starts_with('-'))
}

/// Free Google Translate (the Windows default) over one reused connection, printing timings.
fn translate(texts: &[String]) -> ExitCode {
    if texts.is_empty() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    let settings = yiime_core::ApiSettings::default();
    let mut failed = false;
    for text in texts {
        let outcome = yiime_core::translate(&settings, text);
        for attempt in &outcome.attempts {
            eprintln!("{}", attempt.log_line());
        }
        match outcome.result {
            Ok(english) => println!("{english}"),
            Err(error) => {
                eprintln!("翻译失败：{error}");
                failed = true;
            }
        }
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::option_value;

    #[test]
    fn option_value_rejects_missing_and_flags() {
        let path = "words.txt".to_owned();
        let flag = "--tsv".to_owned();
        assert_eq!(option_value(Some(&path)), Some("words.txt"));
        assert_eq!(option_value(Some(&flag)), None);
        assert_eq!(option_value(Some(&String::new())), None);
        assert_eq!(option_value(None), None);
    }
}
