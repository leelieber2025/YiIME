#!/usr/bin/env python3
"""生成五笔 86 补充词库 data/wubi_extra/wubi86_extra.dict.yaml。

来源（均在生成时按固定快照下载，见 data/README.md）：
- 拼音词库 data/pinyin/pinyin_simp.dict.yaml（rime-pinyin-simp，Apache-2.0）中的多字词；
- 成语 chinese-xinhua idiom.json（MIT）；
- 极点五笔 wubi86_jidian.dict.yaml（KyleBing/rime-wubi86-jidian，MIT）中 rime 五笔词库缺少的条目。
多字词的五笔码按标准取码规则由单字全码计算（单字全码取自 data/wubi/wubi86.dict.yaml）：
二字词取各字前两码；三字词取前两字首码加第三字前两码；四字及以上取前三字首码加末字首码。
已在 rime 五笔词库中的「词 + 码」不重复收录。

用法：python3 tools/gen_wubi_extra.py <idiom.json> <wubi86_jidian.dict.yaml>
"""
import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def parse(path):
    body = False
    for line in Path(path).read_text(encoding="utf-8").splitlines():
        if line == "...":
            body = True
            continue
        if not body or line.startswith("#"):
            continue
        fields = line.split("\t")
        if len(fields) >= 2:
            weight = fields[2] if len(fields) > 2 else "0"
            try:
                weight = float(weight)
            except ValueError:
                weight = 0.0
            yield fields[0], fields[1], weight


def han(text):
    return all("\u3400" <= ch <= "\u9fff" or "\U00020000" <= ch <= "\U0002ffff" for ch in text)


def word_code(word, full):
    codes = [full.get(ch) for ch in word]
    if len(word) < 2 or any(code is None for code in codes):
        return None
    if len(word) == 2:
        code = codes[0][:2] + codes[1][:2]
    elif len(word) == 3:
        code = codes[0][:1] + codes[1][:1] + codes[2][:2]
    else:
        code = codes[0][:1] + codes[1][:1] + codes[2][:1] + codes[-1][:1]
    return code if len(code) == 4 else None


def main():
    idiom_path, jidian_path = sys.argv[1], sys.argv[2]
    rime = list(parse(ROOT / "data/wubi/wubi86.dict.yaml"))
    existing = {(text, code) for text, code, _ in rime}
    existing_texts = {text for text, _, _ in rime}
    # Full code of a character: its longest code, the most frequent one among equal lengths.
    full = {}
    for text, code, weight in rime:
        if len(text) == 1:
            best = full.get(text)
            if best is None or (len(code), weight) > (len(best[0]), best[1]):
                full[text] = (code, weight)
    full = {text: code for text, (code, _) in full.items()}

    # Check the word-code rules against the multi-character words in the rime dictionary.
    checked = matched = 0
    rime_codes = {}
    for text, code, _ in rime:
        rime_codes.setdefault(text, set()).add(code)
    for text, codes in rime_codes.items():
        if len(text) >= 2 and han(text):
            generated = word_code(text, full)
            if generated:
                checked += 1
                matched += generated in codes

    extra = {}  # (text, code) -> (weight, source)

    def add(text, code, weight, source):
        key = (text, code)
        if key in existing:
            return
        if key not in extra or extra[key][0] < weight:
            extra[key] = (weight, source)

    for text, _, weight in parse(ROOT / "data/pinyin/pinyin_simp.dict.yaml"):
        if len(text) >= 2 and han(text):
            code = word_code(text, full)
            if code:
                add(text, code, max(weight, 1.0), "pinyin")
    for item in json.loads(Path(idiom_path).read_text(encoding="utf-8")):
        text = item["word"].strip()
        if len(text) >= 2 and han(text):
            code = word_code(text, full)
            if code:
                add(text, code, 1.0, "idiom")
    for text, code, _ in parse(jidian_path):
        if han(text) and code.isascii() and code.isalpha() and not code.startswith("z"):
            add(text, code, 1.0, "jidian")

    out = ROOT / "data/wubi_extra/wubi86_extra.dict.yaml"
    out.parent.mkdir(parents=True, exist_ok=True)
    lines = [
        "# 由 tools/gen_wubi_extra.py 生成，请勿手工编辑。来源与许可见 data/README.md。",
        "---",
        "name: wubi86_extra",
        'version: "1"',
        "...",
    ]
    for (text, code), (weight, _) in sorted(extra.items(), key=lambda kv: (kv[0][1], -kv[1][0], kv[0][0])):
        lines.append(f"{text}\t{code}\t{int(weight)}")
    out.write_text("\n".join(lines) + "\n", encoding="utf-8")

    sources = {}
    for (text, _), (_, source) in extra.items():
        sources[source] = sources.get(source, 0) + 1
    idioms = {item["word"] for item in json.loads(Path(idiom_path).read_text(encoding="utf-8"))}
    all_texts = existing_texts | {text for text, _ in extra}
    print(f"rule check against rime words: {matched}/{checked} = {matched / checked:.2%}")
    print(f"extra entries: {len(extra)} by source {sources}")
    print(f"new texts not in rime: {len({t for t, _ in extra} - existing_texts)}")
    print(f"total distinct words (>=2 chars) after merge: {sum(1 for t in all_texts if len(t) >= 2)}")
    print(f"idioms covered: {len(idioms & all_texts)} of {len(idioms)}")
    for sample in ["一心一意", "画蛇添足", "翻译", "输入法", "中华人民共和国"]:
        print(sample, word_code(sample, full), sorted(rime_codes.get(sample, [])))


main()
