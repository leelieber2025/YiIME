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
  大词库默认查找顺序：--words、环境变量 YIIME_WORDS、程序旁的 YiIME-words.txt、
  上级目录中的 data/frost/YiIME-words.txt 或 share/yiime/YiIME-words.txt（旧名 InputTranslation-words.txt 也认）。
  --words 的路径不能以 - 开头（写成 ./-文件名）。
  默认与输入法一样只显示常用字（不出扩展区生僻字、低频简拼词）；--all-chars 显示全部。";

const WORDS_FILE: &str = "YiIME-words.txt";
/// Name before 0.2 (still found next to older installs).
const LEGACY_WORDS_FILE: &str = "InputTranslation-words.txt";

fn find_words() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("YIIME_WORDS") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    exe.ancestors().skip(1).find_map(|dir| {
        [WORDS_FILE, LEGACY_WORDS_FILE]
            .into_iter()
            .find_map(|name| {
                [
                    dir.join(name),
                    dir.join("data").join("frost").join(name),
                    dir.join("share").join("yiime").join(name),
                ]
                .into_iter()
                .find(|path| path.is_file())
            })
    })
}

fn main() -> ExitCode {
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    run(&args)
}

fn run(args: &[std::ffi::OsString]) -> ExitCode {
    let sub = args.first().and_then(|s| s.to_str());
    if sub == Some("translate") {
        let mut texts = Vec::with_capacity(args.len().saturating_sub(1));
        for arg in &args[1..] {
            let Some(text) = arg.to_str() else {
                eprintln!("translate 参数不是有效的 UTF-8");
                return ExitCode::from(2);
            };
            texts.push(text.to_owned());
        }
        return translate(&texts);
    }
    if sub == Some("import") {
        return import(&args[1..]);
    }
    if sub != Some("query") {
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
        let Some(arg_str) = arg.to_str() else {
            return fail("query 参数不是有效的 UTF-8");
        };
        match arg_str {
            "--page-size" | "--pages" => {
                let Some(value) = rest
                    .next()
                    .and_then(|s| s.to_str())
                    .and_then(|value| value.parse::<usize>().ok())
                else {
                    return fail(&format!("{arg_str} 需要一个数字"));
                };
                if arg_str == "--pages" {
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
                let Some(scheme) = rest
                    .next()
                    .and_then(|s| s.to_str())
                    .and_then(Shuangpin::from_id)
                else {
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
fn import(args: &[std::ffi::OsString]) -> ExitCode {
    let (file, into) = match args {
        [file] => (std::path::Path::new(file), None),
        [file, flag, into] if flag.to_str() == Some("--into") => {
            (std::path::Path::new(file), Some(std::path::Path::new(into)))
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let words = match std::fs::read(file) {
        Ok(bytes) => match yiime_core::import_words(&bytes) {
            Ok(words) => words,
            Err(error) => {
                eprintln!("{}：{error}", file.display());
                return ExitCode::FAILURE;
            }
        },
        Err(error) => {
            eprintln!("读取 {} 失败：{error}", file.display());
            return ExitCode::FAILURE;
        }
    };
    let Some(into) = into else {
        for (word, code) in &words {
            println!("{word} {code}");
        }
        return ExitCode::SUCCESS;
    };
    let added = match yiime_core::import_into(into, &words) {
        Ok(added) => added,
        Err(error) => {
            eprintln!("写入 {} 失败：{error}", into.display());
            return ExitCode::FAILURE;
        }
    };
    eprintln!("已导入 {added} 个新词（共读到 {} 个）", words.len());
    ExitCode::SUCCESS
}

/// Value of an option that takes a path: missing or another option (`-…`) is an error.
fn option_value(value: Option<&std::ffi::OsString>) -> Option<&std::ffi::OsStr> {
    value
        .map(std::ffi::OsString::as_os_str)
        .filter(|value| !value.is_empty() && !value.to_str().is_some_and(|s| s.starts_with('-')))
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
        use std::ffi::OsString;
        let path = OsString::from("words.txt");
        let flag = OsString::from("--tsv");
        assert_eq!(option_value(Some(&path)), Some(path.as_os_str()));
        assert_eq!(option_value(Some(&flag)), None);
        assert_eq!(option_value(Some(&OsString::new())), None);
        assert_eq!(option_value(None), None);
    }

    #[cfg(unix)]
    #[test]
    fn import_accepts_non_utf8_paths() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        let dir = std::env::temp_dir().join(format!("yiime-cli-nonutf8-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join(OsString::from_vec(b"src-\xff.txt".to_vec()));
        let dst = dir.join(OsString::from_vec(b"dst-\xfe.txt".to_vec()));
        std::fs::write(&src, "你好 nihao\n").unwrap();
        let code = super::run(&[
            OsString::from("import"),
            src.into_os_string(),
            OsString::from("--into"),
            dst.clone().into_os_string(),
        ]);
        assert_eq!(code, std::process::ExitCode::SUCCESS);
        let saved = std::fs::read_to_string(&dst).unwrap();
        assert!(saved.contains("你好 nihao"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
