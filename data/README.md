# 词库来源

- `wubi/wubi86.dict.yaml`：[`rime/rime-wubi`](https://github.com/rime/rime-wubi)，快照 `152a0d3f3efe40cae216d1e3b338242446848d07`，LGPL-3.0。原作者见 `wubi/AUTHORS`，许可见 `wubi/LICENSE`。
- `pinyin/pinyin_simp.dict.yaml`：[`rime/rime-pinyin-simp`](https://github.com/rime/rime-pinyin-simp)，快照 `0c6861ef7420ee780270ca6d993d18d4101049d0`，Apache-2.0。原作者见 `pinyin/AUTHORS`，许可见 `pinyin/LICENSE`。

- `wubi_extra/wubi86_extra.dict.yaml`：五笔 86 补充词库（多字词与成语），由 [`tools/gen_wubi_extra.py`](../tools/gen_wubi_extra.py) 生成。
  多字词的五笔码按标准取码规则（二字词各取前两码；三字词取前两字首码加第三字前两码；四字及以上取前三字首码加末字首码）由上面 rime 五笔词库的单字全码计算；用 rime 五笔词库自带的 61194 个多字词校验，规则一致率 99.96%。词条来源：
  - 上面 rime 简体拼音词库中的多字词（Apache-2.0）；
  - 成语：[`pwxcoo/chinese-xinhua`](https://github.com/pwxcoo/chinese-xinhua) `data/idiom.json`，快照 `fe6d6c2e8baa82187f4c96bbe042e43f96c05666`，MIT，许可见 `wubi_extra/LICENSE-chinese-xinhua`；
  - [`KyleBing/rime-wubi86-jidian`](https://github.com/KyleBing/rime-wubi86-jidian) `wubi86_jidian.dict.yaml` 中 rime 五笔词库没有的条目，快照 `4d08110b8080f6577dad733ada7e5ae5c5e878fd`，MIT，许可见 `wubi_extra/LICENSE-rime-wubi86-jidian`。
  补充词条词频低于 rime 五笔词库原有条目，同一编码下原有字词排在前面。

- `frost/YiIME-words.txt`：大词库（30 万条多字词，含 32876 条四字成语），由 [`tools/gen_frost_words.py`](../tools/gen_frost_words.py) 从 [`gaboolic/rime-frost`](https://github.com/gaboolic/rime-frost)（白霜拼音）快照 `0ba3f340b9a638cef41bfb377b2d376f73e3711d` 的 `cn_dicts/base.dict.yaml`、`cn_dicts/ext.dict.yaml`、`cn_dicts_cell/idiom.dict.yaml` 生成：取多字词的拼音与词频，四字成语全部保留，其余按词频取满 30 万条；五笔 86 词码优先直接采用白霜自带五笔词库 `cn_dicts_wb/words.dict.yaml`（同一快照，GPL-3.0）的现成编码（102621 条），其余 197379 条按上面同样的标准取码规则计算。
  **许可：GPL-3.0**（见 `frost/LICENSE`）。该文件作为独立数据文件随包发布，由程序在运行时读取，不编译进 EXE；修改或再分发该文件须遵守 GPL-3.0（附带许可证并提供本说明中的来源）。删除该文件程序仍可正常使用（只是少了大词库）。

- `opencc/STCharacters.txt`、`opencc/STPhrases.txt`：简→繁转换表（繁体输出用，编进程序），来自 [`BYVoid/OpenCC`](https://github.com/BYVoid/OpenCC) 快照 `fae2a0a911c30764e669a7d8b0591cd8e852934a`，Apache-2.0，许可见 `opencc/LICENSE`。词组优先、单字兜底，多个繁体写法时取第一个。


## gb2312

`gb2312/hanzi.txt`：GB/T 2312-1980 的 6763 个汉字（按码点排序），由 Python 标准库的 `gb2312` 编码表生成，
用于“只显示常用字”时把 GB 2312 之外的单字（多为繁体、异体字）排到后面。字符集标准本身不涉及版权。
