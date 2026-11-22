//! Shuangpin (double pinyin): every syllable is two keys, an initial key and a final key. The
//! composer keeps the keys as typed and looks candidates up under the full pinyin spelling from
//! [`Shuangpin::expand`], so dictionaries, learning and initials matching are shared with full
//! pinyin. Supported layouts: 小鹤双拼, 微软双拼 and 自然码.

/// A Shuangpin layout; `id` is what the settings store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shuangpin {
    Xiaohe,
    Microsoft,
    Ziranma,
}

/// Valid syllables (ü written as v after l/n, as the bundled dictionaries do).
const SYLLABLES: &str = "a ai an ang ao ba bai ban bang bao bei ben beng bi bian biao bie bin bing \
bo bu ca cai can cang cao ce cen ceng cha chai chan chang chao che chen cheng chi chong chou chu \
chua chuai chuan chuang chui chun chuo ci cong cou cu cuan cui cun cuo da dai dan dang dao de dei \
den deng di dia dian diao die ding diu dong dou du duan dui dun duo e ei en eng er fa fan fang fei \
fen feng fo fou fu ga gai gan gang gao ge gei gen geng gong gou gu gua guai guan guang gui gun guo \
ha hai han hang hao he hei hen heng hong hou hu hua huai huan huang hui hun huo ji jia jian jiang \
jiao jie jin jing jiong jiu ju juan jue jun ka kai kan kang kao ke kei ken keng kong kou ku kua \
kuai kuan kuang kui kun kuo la lai lan lang lao le lei leng li lia lian liang liao lie lin ling liu \
lo long lou lu luan lue lun luo lv ma mai man mang mao me mei men meng mi mian miao mie min ming \
miu mo mou mu na nai nan nang nao ne nei nen neng ni nian niang niao nie nin ning niu nong nou nu \
nuan nue nuo nv o ou pa pai pan pang pao pei pen peng pi pian piao pie pin ping po pou pu qi qia \
qian qiang qiao qie qin qing qiong qiu qu quan que qun ran rang rao re ren reng ri rong rou ru ruan \
rui run ruo sa sai san sang sao se sen seng sha shai shan shang shao she shei shen sheng shi shou \
shu shua shuai shuan shuang shui shun shuo si song sou su suan sui sun suo ta tai tan tang tao te \
tei teng ti tian tiao tie ting tong tou tu tuan tui tun tuo wa wai wan wang wei wen weng wo wu xi \
xia xian xiang xiao xie xin xing xiong xiu xu xuan xue xun ya yan yang yao ye yi yin ying yo yong \
you yu yuan yue yun za zai zan zang zao ze zei zen zeng zha zhai zhan zhang zhao zhe zhei zhen \
zheng zhi zhong zhou zhu zhua zhuai zhuan zhuang zhui zhun zhuo zi zong zou zu zuan zui zun zuo";

fn is_syllable(text: &str) -> bool {
    SYLLABLES.split(' ').any(|syllable| syllable == text)
}

impl Shuangpin {
    pub const ALL: [Self; 3] = [Self::Xiaohe, Self::Microsoft, Self::Ziranma];

    pub fn id(self) -> &'static str {
        match self {
            Self::Xiaohe => "xiaohe",
            Self::Microsoft => "microsoft",
            Self::Ziranma => "ziranma",
        }
    }
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|scheme| scheme.id() == id)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Xiaohe => "小鹤双拼",
            Self::Microsoft => "微软双拼",
            Self::Ziranma => "自然码双拼",
        }
    }
    /// Whether `key` is a code key in this layout (微软双拼 uses `;` for -ing).
    pub fn is_key(self, key: char) -> bool {
        key.is_ascii_lowercase() || (key == ';' && self == Self::Microsoft)
    }

    fn initial(self, key: char) -> Option<&'static str> {
        Some(match key {
            'v' => "zh",
            'i' => "ch",
            'u' => "sh",
            'b' => "b",
            'p' => "p",
            'm' => "m",
            'f' => "f",
            'd' => "d",
            't' => "t",
            'n' => "n",
            'l' => "l",
            'g' => "g",
            'k' => "k",
            'h' => "h",
            'j' => "j",
            'q' => "q",
            'x' => "x",
            'r' => "r",
            'z' => "z",
            'c' => "c",
            's' => "s",
            'y' => "y",
            'w' => "w",
            _ => return None,
        })
    }

    /// Finals a key may stand for (the right one is the one that makes a valid syllable).
    fn finals(self, key: char) -> &'static [&'static str] {
        match (self, key) {
            (_, 'a') => &["a"],
            (_, 'e') => &["e"],
            (_, 'i') => &["i"],
            (_, 'u') => &["u"],
            (_, 'o') => &["o", "uo"],
            (_, 'q') => &["iu"],
            (_, 'r') => &["uan"],
            (_, 's') => &["ong", "iong"],
            (_, 'f') => &["en"],
            (_, 'g') => &["eng"],
            (_, 'h') => &["ang"],
            (_, 'j') => &["an"],
            (_, 'm') => &["ian"],
            (Self::Xiaohe, 'w') => &["ei"],
            (Self::Xiaohe, 't') => &["ue", "ve"],
            (Self::Xiaohe, 'y') => &["un"],
            (Self::Xiaohe, 'p') => &["ie"],
            (Self::Xiaohe, 'd') => &["ai"],
            (Self::Xiaohe, 'k') => &["ing", "uai"],
            (Self::Xiaohe, 'l') => &["iang", "uang"],
            (Self::Xiaohe, 'z') => &["ou"],
            (Self::Xiaohe, 'x') => &["ia", "ua"],
            (Self::Xiaohe, 'c') => &["ao"],
            (Self::Xiaohe, 'v') => &["ui", "v"],
            (Self::Xiaohe, 'b') => &["in"],
            (Self::Xiaohe, 'n') => &["iao"],
            (Self::Microsoft, 'w') => &["ia", "ua"],
            (Self::Microsoft, 't') => &["ue"],
            (Self::Microsoft, 'y') => &["uai", "v"],
            (Self::Microsoft, 'p') => &["un"],
            (Self::Microsoft, 'd') => &["iang", "uang"],
            (Self::Microsoft, 'k') => &["ao"],
            (Self::Microsoft, 'l') => &["ai"],
            (Self::Microsoft, ';') => &["ing"],
            (Self::Microsoft, 'z') => &["ei"],
            (Self::Microsoft, 'x') => &["ie"],
            (Self::Microsoft, 'c') => &["iao"],
            (Self::Microsoft, 'v') => &["ui", "ve"],
            (Self::Microsoft, 'b') => &["ou"],
            (Self::Microsoft, 'n') => &["in"],
            (Self::Ziranma, 'w') => &["ia", "ua"],
            (Self::Ziranma, 't') => &["ue", "ve"],
            (Self::Ziranma, 'y') => &["ing", "uai"],
            (Self::Ziranma, 'p') => &["un"],
            (Self::Ziranma, 'd') => &["iang", "uang"],
            (Self::Ziranma, 'k') => &["ao"],
            (Self::Ziranma, 'l') => &["ai"],
            (Self::Ziranma, 'z') => &["ei"],
            (Self::Ziranma, 'x') => &["ie"],
            (Self::Ziranma, 'c') => &["iao"],
            (Self::Ziranma, 'v') => &["ui", "v"],
            (Self::Ziranma, 'b') => &["ou"],
            (Self::Ziranma, 'n') => &["in"],
            _ => &[],
        }
    }

    /// One syllable from its two keys; None if the pair is not a valid syllable.
    pub fn syllable(self, first: char, second: char) -> Option<String> {
        // Syllables without an initial: 微软双拼 types them after `o` (`oa` a, `oj` an), the others
        // with the vowel itself (`aa` a, `ai` ai, `ah` ang, `eg` eng, `oo` o, `ou` ou).
        let zero = match self {
            Self::Microsoft => first == 'o',
            _ => matches!(first, 'a' | 'e' | 'o'),
        };
        if zero {
            let vowel = if self == Self::Microsoft {
                None
            } else {
                Some(first)
            };
            if vowel == Some(second) {
                return Some(first.to_string());
            }
            if let Some(vowel) = vowel {
                let direct = format!("{vowel}{second}");
                if is_syllable(&direct) {
                    return Some(direct);
                }
            }
            return self.finals(second).iter().find_map(|fin| {
                let fin = if self == Self::Microsoft && second == 'r' {
                    "er"
                } else {
                    fin
                };
                let whole = match vowel {
                    Some(vowel) if !fin.starts_with(vowel) => return None,
                    _ => fin.to_owned(),
                };
                is_syllable(&whole).then_some(whole)
            });
        }
        let initial = self.initial(first)?;
        self.finals(second).iter().find_map(|fin| {
            // Written forms: ü is v only after l/n (ju, que, xue, yue).
            let fin = match (*fin, initial) {
                ("v", "j" | "q" | "x" | "y") => "u",
                ("ve", "j" | "q" | "x" | "y") => "ue",
                ("ve", "l" | "n") => "ve",
                (fin, _) => fin,
            };
            let whole = format!("{initial}{fin}");
            let whole = match whole.as_str() {
                "lve" => "lue".to_owned(),
                "nve" => "nue".to_owned(),
                _ => whole,
            };
            is_syllable(&whole).then_some(whole)
        })
    }

    /// Syllables for typed keys (`'` ignored); a trailing odd key stays a bare initial. None if
    /// a pair is not a valid syllable.
    pub fn syllables(self, keys: &str) -> Option<Vec<String>> {
        let keys: Vec<char> = keys.chars().filter(|key| *key != '\'').collect();
        let mut result = Vec::new();
        for pair in keys.chunks(2) {
            match *pair {
                [first, second] => result.push(self.syllable(first, second)?),
                [first] => result.push(match self.initial(first) {
                    Some(initial) => initial.to_owned(),
                    None if matches!(first, 'a' | 'e' | 'o') => first.to_string(),
                    None => return None,
                }),
                _ => unreachable!(),
            }
        }
        Some(result)
    }

    /// Full pinyin to look candidates up with: syllables separated by `'`, always ending in `'`
    /// so the code is never read as Wubi (`nihk` → `ni'hao'` in 小鹤双拼).
    pub fn expand(self, keys: &str) -> Option<String> {
        let syllables = self.syllables(keys)?;
        let mut code = String::new();
        for syllable in syllables {
            code.push_str(&syllable);
            code.push('\'');
        }
        Some(code)
    }

    /// The keys left for a partial match whose unconsumed full pinyin is `rest` (letters only):
    /// the key pairs of the trailing syllables spelling `rest`. None if no syllable boundary fits.
    pub fn rest_keys(self, keys: &str, rest: &str) -> Option<String> {
        let syllables = self.syllables(keys)?;
        let keys: Vec<char> = keys.chars().filter(|key| *key != '\'').collect();
        (0..syllables.len()).find_map(|start| {
            (syllables[start..].concat() == rest).then(|| keys[start * 2..].iter().collect())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xiaohe() {
        let s = Shuangpin::Xiaohe;
        assert_eq!(s.expand("nihc").as_deref(), Some("ni'hao'"));
        assert_eq!(s.expand("vsgo").as_deref(), Some("zhong'guo'"));
        assert_eq!(s.expand("uuru").as_deref(), Some("shu'ru'"));
        assert_eq!(s.expand("lvlt").as_deref(), Some("lv'lue'"));
        assert_eq!(s.expand("jvxt").as_deref(), Some("ju'xue'"));
        assert_eq!(s.expand("kkll").as_deref(), Some("kuai'liang'"));
        assert_eq!(
            s.expand("aaahegeroozz").as_deref(),
            Some("a'ang'eng'er'o'zou'")
        );
        assert_eq!(s.expand("ni'h").as_deref(), Some("ni'h'"));
        assert_eq!(s.expand("xb").as_deref(), Some("xin'"));
        assert_eq!(s.expand("bq"), None, "biu is not a syllable");
    }

    #[test]
    fn microsoft() {
        let s = Shuangpin::Microsoft;
        assert_eq!(s.expand("nihk").as_deref(), Some("ni'hao'"));
        assert_eq!(s.expand("b;").as_deref(), Some("bing'"));
        assert_eq!(s.expand("oaojoror").as_deref(), Some("a'an'er'er'"));
        assert_eq!(s.expand("lyvy").as_deref(), Some("lv'zhuai'"));
        assert_eq!(s.expand("xvlv").as_deref(), Some("xue'lue'"));
        assert!(s.is_key(';') && !Shuangpin::Xiaohe.is_key(';'));
    }

    #[test]
    fn ziranma() {
        let s = Shuangpin::Ziranma;
        assert_eq!(s.expand("nihk").as_deref(), Some("ni'hao'"));
        assert_eq!(s.expand("by").as_deref(), Some("bing'"));
        assert_eq!(s.expand("uyvd").as_deref(), Some("shuai'zhuang'"));
        assert_eq!(s.expand("ahou").as_deref(), Some("ang'ou'"));
        assert_eq!(Shuangpin::from_id("ziranma"), Some(s));
        assert_eq!(Shuangpin::from_id("quanpin"), None);
    }

    #[test]
    fn rest_keys_follow_syllable_boundaries() {
        let s = Shuangpin::Xiaohe;
        assert_eq!(s.rest_keys("nihcuuru", "shuru").as_deref(), Some("uuru"));
        assert_eq!(s.rest_keys("nihcuuru", "ru").as_deref(), Some("ru"));
        assert_eq!(s.rest_keys("nihcuuru", "hru"), None);
    }
}
