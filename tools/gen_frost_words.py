#!/usr/bin/env python3
"""生成运行时词库 data/frost/YiIME-words.txt（GPL-3.0，单独文件，不编进 EXE）。

来源：gaboolic/rime-frost（GPL-3.0）cn_dicts/base.dict.yaml、cn_dicts/ext.dict.yaml、
cn_dicts_cell/idiom.dict.yaml 中的多字词，按词频取前 CAP 个。
每行：词<TAB>拼音（空格分隔）<TAB>词频<TAB>五笔86词码。
五笔词码优先直接取白霜自带的五笔词库 cn_dicts_wb/words.dict.yaml（同为 GPL-3.0）；
其中没有的词才按标准规则由 data/wubi/wubi86.dict.yaml 的单字全码计算（与 gen_wubi_extra.py 相同）。

用法：python3 tools/gen_frost_words.py <frost 目录> [CAP]
（frost 目录内需有 base/ext/idiom 三个 dict.yaml 及 wb_words.dict.yaml = cn_dicts_wb/words.dict.yaml）
"""
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.argv_backup = list(sys.argv)


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
            try:
                weight = float(fields[2]) if len(fields) > 2 and fields[2] else 0.0
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
    frost = Path(sys.argv[1])
    cap = int(sys.argv[2]) if len(sys.argv) > 2 else 300_000
    full = {}
    for text, code, weight in parse(ROOT / "data/wubi/wubi86.dict.yaml"):
        if len(text) == 1:
            best = full.get(text)
            if best is None or (len(code), weight) > (len(best[0]), best[1]):
                full[text] = (code, weight)
    full = {text: code for text, (code, _) in full.items()}
    ready = {}
    for text, code, weight in parse(frost / "wb_words.dict.yaml"):
        if len(code) == 4 and code.isascii() and code.isalpha():
            best = ready.get(text)
            if best is None or weight > best[1]:
                ready[text] = (code, weight)
    ready = {text: code for text, (code, _) in ready.items()}
    words = {}
    idioms = set()
    for name in ["base.dict.yaml", "ext.dict.yaml", "idiom.dict.yaml"]:
        for text, pinyin, weight in parse(frost / name):
            if len(text) < 2 or not han(text):
                continue
            syllables = pinyin.split()
            if len(syllables) != len(text) or not all(s.isascii() and s.isalpha() for s in syllables):
                continue
            code = ready.get(text) or word_code(text, full)
            if not code:
                continue
            if name == "idiom.dict.yaml":
                idioms.add(text)
            old = words.get(text)
            if old is None or weight > old[1]:
                words[text] = (" ".join(syllables), weight, code)
    # Keep every four-character idiom (idiom frequencies are low and a frequency cut would drop
    # them), then fill up to CAP by frequency.
    ordered = sorted(words.items(), key=lambda kv: (-kv[1][1], kv[0]))
    must = [item for item in ordered if item[0] in idioms and len(item[0]) == 4]
    must_set = {text for text, _ in must}
    rest = [item for item in ordered if item[0] not in must_set][: max(cap - len(must), 0)]
    ranked = sorted(must + rest, key=lambda kv: (-kv[1][1], kv[0]))
    out = ROOT / "data/frost/YiIME-words.txt"
    out.parent.mkdir(parents=True, exist_ok=True)
    with out.open("w", encoding="utf-8", newline="\n") as handle:
        handle.write("# 词<TAB>拼音<TAB>词频<TAB>五笔86词码。来源 gaboolic/rime-frost（GPL-3.0），见 data/README.md。\n")
        for text, (pinyin, weight, code) in ranked:
            handle.write(f"{text}\t{pinyin}\t{int(weight)}\t{code}\n")
    kept = {text for text, _ in ranked}
    from_ready = sum(1 for text, (_, _, code) in ranked if ready.get(text) == code)
    differs = sum(1 for text, _ in ranked if text in ready and word_code(text, full) not in (None, ready[text]))
    print(f"wubi codes: ready-made {from_ready}, derived {len(ranked) - from_ready}, "
          f"ready-made differing from rule {differs}")
    print(f"candidates {len(words)}, kept {len(ranked)} (cap {cap}), idioms kept {len(kept & idioms)}, "
          f"four-char kept {sum(1 for t in kept if len(t) == 4)}, size {out.stat().st_size} bytes, "
          f"min weight kept {ranked[-1][1][1] if ranked else 0}")


main()
