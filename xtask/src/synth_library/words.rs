// SPDX-FileCopyrightText: 2026 sorrelfox and Helikodu contributors
// SPDX-License-Identifier: GPL-3.0-or-later

//! Word lists in several scripts, and names made from them.
//!
//! Every name is made up by combining common words and given and family
//! names; none is meant to be a real artist or work. The Latin lists carry
//! the letters that sort differently by locale (Estonian `õ ä ö ü š ž`,
//! Swedish `å ä ö`, German `ä ö ü ß`).

use super::random::Rng;

/// The script, and with it the language, of a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Script {
    English,
    Estonian,
    Swedish,
    German,
    /// French, Czech, Polish, Spanish: diacritics outside the three above.
    OtherLatin,
    Cyrillic,
    Greek,
    Chinese,
    Japanese,
    Korean,
    Thai,
    Arabic,
    Hebrew,
}

impl Script {
    #[cfg(test)]
    pub(crate) const ALL: [Script; 13] = [
        Script::English,
        Script::Estonian,
        Script::Swedish,
        Script::German,
        Script::OtherLatin,
        Script::Cyrillic,
        Script::Greek,
        Script::Chinese,
        Script::Japanese,
        Script::Korean,
        Script::Thai,
        Script::Arabic,
        Script::Hebrew,
    ];

    /// How often an artist uses each script.
    pub(crate) const WEIGHTS: [(Script, u32); 13] = [
        (Script::English, 45),
        (Script::Estonian, 6),
        (Script::Swedish, 6),
        (Script::German, 6),
        (Script::OtherLatin, 5),
        (Script::Cyrillic, 8),
        (Script::Greek, 3),
        (Script::Chinese, 5),
        (Script::Japanese, 6),
        (Script::Korean, 4),
        (Script::Thai, 2),
        (Script::Arabic, 2),
        (Script::Hebrew, 2),
    ];

    /// Whether words are written without spaces between them.
    fn joins_without_space(self) -> bool {
        matches!(self, Script::Chinese | Script::Japanese)
    }

    /// Whether the first letter of a word can be made upper case.
    fn has_case(self) -> bool {
        matches!(
            self,
            Script::English
                | Script::Estonian
                | Script::Swedish
                | Script::German
                | Script::OtherLatin
                | Script::Cyrillic
                | Script::Greek
        )
    }

    fn lists(self) -> &'static Lists {
        match self {
            Script::English => &ENGLISH,
            Script::Estonian => &ESTONIAN,
            Script::Swedish => &SWEDISH,
            Script::German => &GERMAN,
            Script::OtherLatin => &OTHER_LATIN,
            Script::Cyrillic => &CYRILLIC,
            Script::Greek => &GREEK,
            Script::Chinese => &CHINESE,
            Script::Japanese => &JAPANESE,
            Script::Korean => &KOREAN,
            Script::Thai => &THAI,
            Script::Arabic => &ARABIC,
            Script::Hebrew => &HEBREW,
        }
    }
}

#[derive(Debug)]
struct Lists {
    /// Given names, with a Latin transcription for scripts that are often
    /// sorted by one (empty otherwise).
    given: &'static [(&'static str, &'static str)],
    family: &'static [(&'static str, &'static str)],
    words: &'static [&'static str],
    /// Whether the family name comes first (Chinese, Japanese, Korean).
    family_first: bool,
}

/// A person's or a band's name, with the sort form a careful tagger would
/// write.
#[derive(Debug, Clone)]
pub(crate) struct Name {
    pub(crate) text: String,
    pub(crate) sort: Option<String>,
    pub(crate) script: Script,
}

/// A person: "Given Family", or "Family Given" where that is the custom.
/// Latin names get the sort form "Family, Given" with probability
/// `sort_share`; Japanese names get a romanised sort form as often.
pub(crate) fn person(rng: &mut Rng, script: Script, sort_share: f64) -> Name {
    let lists = script.lists();
    let (given, given_latin) = *rng.pick(lists.given);
    let (family, family_latin) = *rng.pick(lists.family);
    let separator = if script.joins_without_space() && script != Script::Japanese {
        ""
    } else {
        " "
    };
    let text = if lists.family_first {
        format!("{family}{separator}{given}")
    } else {
        format!("{given}{separator}{family}")
    };
    let sort = if !rng.chance(sort_share) {
        None
    } else if script == Script::Japanese {
        Some(format!("{family_latin} {given_latin}"))
    } else if lists.family_first || !script.has_case() {
        None
    } else {
        Some(format!("{family}, {given}"))
    };
    Name { text, sort, script }
}

/// A band: one to three words, in English often "The …", which always gets
/// the sort form "…, The".
pub(crate) fn band(rng: &mut Rng, script: Script) -> Name {
    if script == Script::English && rng.chance(0.3) {
        let rest = phrase(rng, script, 1, 2);
        return Name {
            text: format!("The {rest}"),
            sort: Some(format!("{rest}, The")),
            script,
        };
    }
    Name {
        text: phrase(rng, script, 1, 3),
        sort: None,
        script,
    }
}

/// `min..=max` words of `script`, capitalised where the script has case.
pub(crate) fn phrase(rng: &mut Rng, script: Script, min: u32, max: u32) -> String {
    let lists = script.lists();
    let count = rng.range(min, max);
    let separator = if script.joins_without_space() {
        ""
    } else {
        " "
    };
    let mut out = String::new();
    for index in 0..count {
        if index > 0 {
            out.push_str(separator);
        }
        let word = *rng.pick(lists.words);
        if script.has_case() && (index == 0 || script == Script::English) {
            out.push_str(&capitalise(word));
        } else {
            out.push_str(word);
        }
    }
    out
}

fn capitalise(word: &str) -> String {
    let mut chars = word.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Genres; a track gets one or, sometimes, several.
pub(crate) const GENRES: &[&str] = &[
    "Rock",
    "Pop",
    "Jazz",
    "Electronic",
    "Hip-Hop",
    "Folk",
    "Blues",
    "Metal",
    "Punk",
    "Ambient",
    "Soul",
    "R&B",
    "Reggae",
    "Country",
    "Indie",
    "Alternative",
    "Techno",
    "House",
    "Drum and Bass",
    "Singer-Songwriter",
    "Soundtrack",
    "World",
    "Funk",
    "Disco",
    "Synth-pop",
    "Post-rock",
    "Shoegaze",
    "J-Pop",
    "K-Pop",
    "Chanson",
    "Schlager",
    "Rebetiko",
    "Luk thung",
    "Experimental",
    "Lo-fi",
];

/// Classical genres and periods.
pub(crate) const CLASSICAL_GENRES: &[&str] = &[
    "Baroque",
    "Romantic",
    "Chamber Music",
    "Opera",
    "Modern Classical",
];

/// Record labels.
pub(crate) const LABELS: &[&str] = &[
    "Northern Lights Records",
    "Kõrvits Helid",
    "Skivbolaget Ö",
    "Schallplatten Süd",
    "Мелодия-Синтез",
    "青空レコード",
    "Disques Éphémères",
    "Blue Fern",
    "Self-released",
];

/// Musical forms for classical works.
pub(crate) const FORMS: &[&str] = &[
    "Symphony",
    "Piano Concerto",
    "Violin Concerto",
    "String Quartet",
    "Piano Sonata",
    "Cello Suite",
    "Requiem",
    "Mass",
    "Serenade",
    "Partita",
];

pub(crate) const KEYS: &[&str] = &[
    "C major",
    "C minor",
    "D major",
    "D minor",
    "E-flat major",
    "E minor",
    "F major",
    "F-sharp minor",
    "G major",
    "G minor",
    "A major",
    "A minor",
    "B-flat major",
    "B minor",
];

pub(crate) const TEMPI: &[&str] = &[
    "Allegro",
    "Allegro con brio",
    "Andante",
    "Adagio",
    "Adagio molto e cantabile",
    "Largo",
    "Presto",
    "Scherzo. Allegro vivace",
    "Menuetto. Allegretto",
    "Rondo. Allegro ma non troppo",
    "Lento",
    "Vivace",
];

pub(crate) const INSTRUMENTS: &[&str] = &[
    "piano",
    "violin",
    "cello",
    "soprano",
    "tenor",
    "baritone",
    "flute",
    "oboe",
    "harpsichord",
];

pub(crate) const ENSEMBLE_KINDS: &[&str] = &[
    "Symphony Orchestra",
    "Philharmonic Orchestra",
    "Chamber Orchestra",
    "String Quartet",
    "Baroque Ensemble",
    "Radio Symphony Orchestra",
];

/// Cities that name orchestras.
pub(crate) const CITIES: &[&str] = &[
    "Tallinn",
    "Tartu",
    "Göteborg",
    "Malmö",
    "München",
    "Köln",
    "Zürich",
    "Praha",
    "Kraków",
    "Lyon",
    "Athens",
    "Helsinki",
    "Tokyo",
    "Wien",
];

/// Titles that many albums share.
pub(crate) const COMMON_ALBUM_TITLES: &[&str] = &[
    "Greatest Hits",
    "Live",
    "Best Of",
    "Unplugged",
    "Demos",
    "Christmas",
    "Untitled",
    "Remixes",
];

/// Titles of compilations.
pub(crate) const COMPILATION_WORDS: &[&str] = &[
    "Summer",
    "Night",
    "Drive",
    "Chill",
    "Classics",
    "Hits",
    "Anthems",
    "Grooves",
    "Lounge",
    "Essentials",
];

/// Generic words for English names.
static ENGLISH: Lists = Lists {
    given: &[
        ("John", ""),
        ("Mary", ""),
        ("David", ""),
        ("Sarah", ""),
        ("Michael", ""),
        ("Emma", ""),
        ("James", ""),
        ("Olivia", ""),
        ("Robert", ""),
        ("Grace", ""),
        ("Daniel", ""),
        ("Lucy", ""),
    ],
    family: &[
        ("Smith", ""),
        ("Johnson", ""),
        ("Brown", ""),
        ("Taylor", ""),
        ("Walker", ""),
        ("Hughes", ""),
        ("Carter", ""),
        ("Bennett", ""),
        ("Fletcher", ""),
        ("Morgan", ""),
        ("O'Neill", ""),
        ("McAllister", ""),
    ],
    words: &[
        "black",
        "silver",
        "electric",
        "river",
        "midnight",
        "glass",
        "paper",
        "lonely",
        "golden",
        "rain",
        "fire",
        "ocean",
        "echo",
        "winter",
        "summer",
        "city",
        "ghost",
        "garden",
        "machine",
        "heart",
        "wolves",
        "stars",
        "satellite",
        "velvet",
        "highway",
        "mirror",
        "dream",
        "shadow",
        "light",
        "storm",
        "orchid",
        "harbour",
        "static",
        "neon",
        "falling",
        "wild",
        "quiet",
        "northern",
        "secret",
        "empire",
    ],
    family_first: false,
};

static ESTONIAN: Lists = Lists {
    given: &[
        ("Jüri", ""),
        ("Tõnu", ""),
        ("Mart", ""),
        ("Ülle", ""),
        ("Kärt", ""),
        ("Õie", ""),
        ("Mari", ""),
        ("Toomas", ""),
        ("Märt", ""),
        ("Liisa", ""),
    ],
    family: &[
        ("Tamm", ""),
        ("Saar", ""),
        ("Mägi", ""),
        ("Kõiv", ""),
        ("Pärn", ""),
        ("Õunapuu", ""),
        ("Järv", ""),
        ("Rätsep", ""),
        ("Šmidt", ""),
        ("Žukov", ""),
        ("Väli", ""),
        ("Zaitsev", ""),
    ],
    words: &[
        "õhtu",
        "öö",
        "järv",
        "mägi",
        "tuul",
        "süda",
        "külm",
        "päike",
        "meri",
        "kuu",
        "lumi",
        "kevad",
        "tähed",
        "valgus",
        "õun",
        "ääremaa",
        "üksi",
        "kõrb",
        "šokolaad",
        "žürii",
        "vihm",
        "laul",
    ],
    family_first: false,
};

static SWEDISH: Lists = Lists {
    given: &[
        ("Åsa", ""),
        ("Björn", ""),
        ("Märta", ""),
        ("Örjan", ""),
        ("Linnéa", ""),
        ("Göran", ""),
        ("Ingrid", ""),
        ("Anders", ""),
        ("Ödön", ""),
    ],
    family: &[
        ("Åkesson", ""),
        ("Öberg", ""),
        ("Ängström", ""),
        ("Lindqvist", ""),
        ("Sjöberg", ""),
        ("Hällström", ""),
        ("Andersson", ""),
        ("Zetterlund", ""),
        ("Östlund", ""),
    ],
    words: &[
        "sjö",
        "ängar",
        "höst",
        "vår",
        "ljus",
        "mörker",
        "ö",
        "strömmar",
        "fjäll",
        "älv",
        "sommar",
        "natt",
        "hav",
        "skog",
        "stjärna",
        "åska",
        "dröm",
        "vinter",
    ],
    family_first: false,
};

static GERMAN: Lists = Lists {
    given: &[
        ("Jürgen", ""),
        ("Bärbel", ""),
        ("Günther", ""),
        ("Jörg", ""),
        ("Käthe", ""),
        ("Ulrike", ""),
        ("Klaus", ""),
        ("Sabine", ""),
    ],
    family: &[
        ("Müller", ""),
        ("Weiß", ""),
        ("Schröder", ""),
        ("Groß", ""),
        ("Hölzl", ""),
        ("Fuchs", ""),
        ("Brückner", ""),
        ("Österreicher", ""),
        ("Zimmermann", ""),
    ],
    words: &[
        "straße",
        "nacht",
        "größe",
        "träume",
        "lärm",
        "fröhlich",
        "weiß",
        "wälder",
        "übergang",
        "mädchen",
        "herz",
        "sonne",
        "stadt",
        "schatten",
        "fluss",
        "maß",
        "äther",
        "öde",
    ],
    family_first: false,
};

static OTHER_LATIN: Lists = Lists {
    given: &[
        ("Hélène", ""),
        ("François", ""),
        ("Cécile", ""),
        ("Noël", ""),
        ("Łukasz", ""),
        ("Zdeněk", ""),
        ("Iñigo", ""),
        ("Agnès", ""),
    ],
    family: &[
        ("Lefèvre", ""),
        ("Dvořák", ""),
        ("Wiśniewski", ""),
        ("Núñez", ""),
        ("Bérenger", ""),
        ("Čapek", ""),
        ("García", ""),
        ("Rousseau", ""),
    ],
    words: &[
        "été",
        "cœur",
        "lumière",
        "rêve",
        "forêt",
        "noël",
        "corazón",
        "canción",
        "mañana",
        "píseň",
        "řeka",
        "noc",
        "żółw",
        "światło",
        "ça",
        "naïve",
    ],
    family_first: false,
};

static CYRILLIC: Lists = Lists {
    given: &[
        ("Алексей", ""),
        ("Мария", ""),
        ("Дмитрий", ""),
        ("Ольга", ""),
        ("Сергей", ""),
        ("Анна", ""),
        ("Юрий", ""),
        ("Ёлка", ""),
    ],
    family: &[
        ("Петров", ""),
        ("Смирнова", ""),
        ("Кузнецов", ""),
        ("Соколова", ""),
        ("Шевченко", ""),
        ("Ёлкин", ""),
        ("Попов", ""),
    ],
    words: &[
        "ночь",
        "звезда",
        "река",
        "ветер",
        "город",
        "песня",
        "зима",
        "свет",
        "море",
        "дорога",
        "сердце",
        "небо",
        "весна",
        "тишина",
        "огонь",
        "ёж",
    ],
    family_first: false,
};

static GREEK: Lists = Lists {
    given: &[
        ("Νίκος", ""),
        ("Ελένη", ""),
        ("Γιώργος", ""),
        ("Μαρία", ""),
        ("Δημήτρης", ""),
        ("Σοφία", ""),
    ],
    family: &[
        ("Παπαδόπουλος", ""),
        ("Οικονόμου", ""),
        ("Νικολάου", ""),
        ("Γεωργίου", ""),
        ("Αλεξίου", ""),
    ],
    words: &[
        "θάλασσα",
        "νύχτα",
        "ήλιος",
        "αγάπη",
        "δρόμος",
        "φως",
        "καρδιά",
        "ουρανός",
        "όνειρο",
        "αέρας",
        "τραγούδι",
    ],
    family_first: false,
};

static CHINESE: Lists = Lists {
    given: &[
        ("明", "Ming"),
        ("伟", "Wei"),
        ("静", "Jing"),
        ("洋", "Yang"),
        ("丽", "Li"),
        ("小龙", "Xiaolong"),
        ("雨涵", "Yuhan"),
    ],
    family: &[
        ("李", "Li"),
        ("王", "Wang"),
        ("张", "Zhang"),
        ("刘", "Liu"),
        ("陈", "Chen"),
        ("黄", "Huang"),
    ],
    words: &[
        "月亮", "夜晚", "城市", "梦", "风", "海", "光", "雨", "春天", "星空", "记忆", "远方", "歌",
        "花",
    ],
    family_first: true,
};

static JAPANESE: Lists = Lists {
    given: &[
        ("太郎", "Tarō"),
        ("花子", "Hanako"),
        ("一郎", "Ichirō"),
        ("美咲", "Misaki"),
        ("健", "Ken"),
        ("さくら", "Sakura"),
        ("翔太", "Shōta"),
    ],
    family: &[
        ("山田", "Yamada"),
        ("佐藤", "Satō"),
        ("鈴木", "Suzuki"),
        ("高橋", "Takahashi"),
        ("田中", "Tanaka"),
        ("渡辺", "Watanabe"),
    ],
    words: &[
        "夜明け",
        "さくら",
        "東京",
        "雨",
        "星",
        "夢",
        "こころ",
        "ひかり",
        "海",
        "風",
        "メロディー",
        "ハート",
        "空",
        "花火",
    ],
    family_first: true,
};

static KOREAN: Lists = Lists {
    given: &[
        ("민준", "Minjun"),
        ("서연", "Seoyeon"),
        ("지훈", "Jihun"),
        ("유진", "Yujin"),
        ("하늘", "Haneul"),
    ],
    family: &[
        ("김", "Kim"),
        ("이", "Lee"),
        ("박", "Park"),
        ("최", "Choi"),
        ("정", "Jeong"),
    ],
    words: &[
        "사랑", "밤", "별", "바다", "하늘", "꿈", "봄", "바람", "노래", "기억", "우리", "너",
    ],
    family_first: true,
};

static THAI: Lists = Lists {
    given: &[
        ("สมชาย", ""),
        ("สุดา", ""),
        ("ประเสริฐ", ""),
        ("มาลี", ""),
        ("วิชัย", ""),
    ],
    family: &[("ใจดี", ""), ("แก้วมณี", ""), ("ศรีสุข", ""), ("ทองดี", "")],
    words: &[
        "ความรัก",
        "ดวงดาว",
        "ทะเล",
        "ฝน",
        "หัวใจ",
        "เพลง",
        "กลางคืน",
        "ท้องฟ้า",
        "ความฝัน",
        "เมือง",
    ],
    family_first: false,
};

static ARABIC: Lists = Lists {
    given: &[
        ("محمد", ""),
        ("ليلى", ""),
        ("عمر", ""),
        ("سارة", ""),
        ("يوسف", ""),
        ("نور", ""),
    ],
    family: &[
        ("العلي", ""),
        ("حسن", ""),
        ("خالد", ""),
        ("المصري", ""),
        ("حداد", ""),
    ],
    words: &[
        "القمر",
        "الليل",
        "البحر",
        "حب",
        "نور",
        "طريق",
        "سماء",
        "قلب",
        "أغنية",
        "صحراء",
    ],
    family_first: false,
};

static HEBREW: Lists = Lists {
    given: &[
        ("דוד", ""),
        ("נועה", ""),
        ("יוסי", ""),
        ("מיכל", ""),
        ("אבי", ""),
        ("שירה", ""),
    ],
    family: &[
        ("כהן", ""),
        ("לוי", ""),
        ("מזרחי", ""),
        ("פרץ", ""),
        ("ביטון", ""),
    ],
    words: &[
        "אהבה", "לילה", "ים", "אור", "שמש", "דרך", "שיר", "לב", "חלום", "עיר",
    ],
    family_first: false,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bands_sort_without_the_article() {
        let mut rng = Rng::new(11);
        let mut seen = false;
        for _ in 0..200 {
            let name = band(&mut rng, Script::English);
            if let Some(rest) = name.text.strip_prefix("The ") {
                assert_eq!(name.sort.as_deref(), Some(format!("{rest}, The").as_str()));
                seen = true;
            }
        }
        assert!(seen);
    }

    #[test]
    fn people_sort_by_family_name() {
        let mut rng = Rng::new(2);
        let name = person(&mut rng, Script::German, 1.0);
        let (given, family) = name.text.split_once(' ').unwrap();
        assert_eq!(name.sort, Some(format!("{family}, {given}")));
    }

    #[test]
    fn japanese_names_sort_by_their_romanisation() {
        let mut rng = Rng::new(2);
        let name = person(&mut rng, Script::Japanese, 1.0);
        let sort = name.sort.unwrap();
        assert!(sort.chars().all(|c| u32::from(c) < 0x250), "{sort}");
    }

    #[test]
    fn every_script_has_words() {
        for script in Script::ALL {
            let lists = script.lists();
            assert!(!lists.given.is_empty() && !lists.family.is_empty());
            assert!(lists.words.len() >= 8, "{script:?}");
            assert!(Script::WEIGHTS.iter().any(|(s, _)| *s == script));
        }
    }
}
