//! A join-order workload over an IMDB-shaped schema (RULES.md D308): 100
//! queries of the project's own (`workload.sql`), each a star or a chain
//! of 4 to 17 tables joined by their foreign keys, with selective filters
//! and MIN of a few columns, over a database generated in the shape of
//! the public IMDB data. The joins benchmark times them
//! (`bench/joins.rs`); the integration tests check that every query gives
//! the same rows however its joins are ordered
//! (`test/integration/join_order_workload.rs`).
//!
//! The data is made from a fixed seed, so every run reads the same rows.
//! Its sizes follow IMDB's ratios to the number of titles, and it is
//! skewed and correlated as IMDB is, which is what makes the benchmark's
//! estimates hard: the rows of the fact tables pick their movie and their
//! person with a strong skew toward a few; each title has a theme, which
//! the values of its rows in every table follow, so predicates on two
//! tables of one movie are correlated; and each string column takes, most of
//! the time, a value of those the queries compare it with, skewed toward
//! the first, so the queries find rows.

use std::collections::HashMap;

use turborel::db::Database;

/// The queries, each after a line naming it.
pub const QUERIES: &str = include_str!("workload.sql");

/// The tables, as the benchmark's `schema.sql` declares them, `text` as a
/// varying string.
const SCHEMA: &str = "CREATE SCHEMA AUTHORIZATION IMDB
  CREATE TABLE aka_name (id INTEGER NOT NULL PRIMARY KEY, person_id INTEGER NOT NULL, name VARCHAR(200) NOT NULL, imdb_index VARCHAR(12), name_pcode_cf VARCHAR(5), name_pcode_nf VARCHAR(5), surname_pcode VARCHAR(5), md5sum VARCHAR(32))
  CREATE TABLE aka_title (id INTEGER NOT NULL PRIMARY KEY, movie_id INTEGER NOT NULL, title VARCHAR(200) NOT NULL, imdb_index VARCHAR(12), kind_id INTEGER NOT NULL, production_year INTEGER, phonetic_code VARCHAR(5), episode_of_id INTEGER, season_nr INTEGER, episode_nr INTEGER, note VARCHAR(200), md5sum VARCHAR(32))
  CREATE TABLE cast_info (id INTEGER NOT NULL PRIMARY KEY, person_id INTEGER NOT NULL, movie_id INTEGER NOT NULL, person_role_id INTEGER, note VARCHAR(200), nr_order INTEGER, role_id INTEGER NOT NULL)
  CREATE TABLE char_name (id INTEGER NOT NULL PRIMARY KEY, name VARCHAR(200) NOT NULL, imdb_index VARCHAR(12), imdb_id INTEGER, name_pcode_nf VARCHAR(5), surname_pcode VARCHAR(5), md5sum VARCHAR(32))
  CREATE TABLE comp_cast_type (id INTEGER NOT NULL PRIMARY KEY, kind VARCHAR(32) NOT NULL)
  CREATE TABLE company_name (id INTEGER NOT NULL PRIMARY KEY, name VARCHAR(200) NOT NULL, country_code VARCHAR(255), imdb_id INTEGER, name_pcode_nf VARCHAR(5), name_pcode_sf VARCHAR(5), md5sum VARCHAR(32))
  CREATE TABLE company_type (id INTEGER NOT NULL PRIMARY KEY, kind VARCHAR(32) NOT NULL)
  CREATE TABLE complete_cast (id INTEGER NOT NULL PRIMARY KEY, movie_id INTEGER, subject_id INTEGER NOT NULL, status_id INTEGER NOT NULL)
  CREATE TABLE info_type (id INTEGER NOT NULL PRIMARY KEY, info VARCHAR(32) NOT NULL)
  CREATE TABLE keyword (id INTEGER NOT NULL PRIMARY KEY, keyword VARCHAR(200) NOT NULL, phonetic_code VARCHAR(5))
  CREATE TABLE kind_type (id INTEGER NOT NULL PRIMARY KEY, kind VARCHAR(15) NOT NULL)
  CREATE TABLE link_type (id INTEGER NOT NULL PRIMARY KEY, link VARCHAR(32) NOT NULL)
  CREATE TABLE movie_companies (id INTEGER NOT NULL PRIMARY KEY, movie_id INTEGER NOT NULL, company_id INTEGER NOT NULL, company_type_id INTEGER NOT NULL, note VARCHAR(200))
  CREATE TABLE movie_info (id INTEGER NOT NULL PRIMARY KEY, movie_id INTEGER NOT NULL, info_type_id INTEGER NOT NULL, info VARCHAR(200) NOT NULL, note VARCHAR(200))
  CREATE TABLE movie_info_idx (id INTEGER NOT NULL PRIMARY KEY, movie_id INTEGER NOT NULL, info_type_id INTEGER NOT NULL, info VARCHAR(200) NOT NULL, note VARCHAR(200))
  CREATE TABLE movie_keyword (id INTEGER NOT NULL PRIMARY KEY, movie_id INTEGER NOT NULL, keyword_id INTEGER NOT NULL)
  CREATE TABLE movie_link (id INTEGER NOT NULL PRIMARY KEY, movie_id INTEGER NOT NULL, linked_movie_id INTEGER NOT NULL, link_type_id INTEGER NOT NULL)
  CREATE TABLE name (id INTEGER NOT NULL PRIMARY KEY, name VARCHAR(200) NOT NULL, imdb_index VARCHAR(12), imdb_id INTEGER, gender VARCHAR(1), name_pcode_cf VARCHAR(5), name_pcode_nf VARCHAR(5), surname_pcode VARCHAR(5), md5sum VARCHAR(32))
  CREATE TABLE person_info (id INTEGER NOT NULL PRIMARY KEY, person_id INTEGER NOT NULL, info_type_id INTEGER NOT NULL, info VARCHAR(200) NOT NULL, note VARCHAR(200))
  CREATE TABLE role_type (id INTEGER NOT NULL PRIMARY KEY, role VARCHAR(32) NOT NULL)
  CREATE TABLE title (id INTEGER NOT NULL PRIMARY KEY, title VARCHAR(200) NOT NULL, imdb_index VARCHAR(12), kind_id INTEGER NOT NULL, production_year INTEGER, imdb_id INTEGER, phonetic_code VARCHAR(5), episode_of_id INTEGER, season_nr INTEGER, episode_nr INTEGER, series_years VARCHAR(49), md5sum VARCHAR(32))";

/// The benchmark's indexes of foreign keys (its `fkindexes.sql`).
const INDEXES: [(&str, &str); 23] = [
    ("movie_companies", "company_id"),
    ("movie_companies", "company_type_id"),
    ("movie_info_idx", "info_type_id"),
    ("movie_info", "info_type_id"),
    ("person_info", "info_type_id"),
    ("movie_keyword", "keyword_id"),
    ("aka_title", "kind_id"),
    ("title", "kind_id"),
    ("movie_link", "linked_movie_id"),
    ("movie_link", "link_type_id"),
    ("aka_title", "movie_id"),
    ("cast_info", "movie_id"),
    ("complete_cast", "movie_id"),
    ("movie_companies", "movie_id"),
    ("movie_info_idx", "movie_id"),
    ("movie_keyword", "movie_id"),
    ("movie_link", "movie_id"),
    ("movie_info", "movie_id"),
    ("aka_name", "person_id"),
    ("cast_info", "person_id"),
    ("person_info", "person_id"),
    ("cast_info", "person_role_id"),
    ("cast_info", "role_id"),
];

/// How a column's values are made.
#[derive(Clone, Copy)]
enum Col {
    /// The row's number.
    Id,
    /// The id of a row of the table named, skewed toward the first.
    Ref(&'static str),
    /// A year, later for later themes.
    Year,
    /// A number below the one given.
    Int(u64),
    /// A string of at most the length given, from the queries' values.
    Str(usize),
}

/// A table to make: its name, its rows per hundred titles (or its fixed
/// rows, of a table of kinds), and its columns, each with the percent of
/// its values that are null.
struct Table {
    name: &'static str,
    per_hundred: u64,
    fixed: bool,
    columns: &'static [(&'static str, Col, u64)],
}

/// The tables, each after those it refers to.
const TABLES: [Table; 21] = [
    Table { name: "kind_type", per_hundred: 7, fixed: true, columns: &[("id", Col::Id, 0), ("kind", Col::Str(15), 0)] },
    Table { name: "company_type", per_hundred: 4, fixed: true, columns: &[("id", Col::Id, 0), ("kind", Col::Str(32), 0)] },
    Table { name: "comp_cast_type", per_hundred: 4, fixed: true, columns: &[("id", Col::Id, 0), ("kind", Col::Str(32), 0)] },
    Table { name: "info_type", per_hundred: 113, fixed: true, columns: &[("id", Col::Id, 0), ("info", Col::Str(32), 0)] },
    Table { name: "link_type", per_hundred: 18, fixed: true, columns: &[("id", Col::Id, 0), ("link", Col::Str(32), 0)] },
    Table { name: "role_type", per_hundred: 12, fixed: true, columns: &[("id", Col::Id, 0), ("role", Col::Str(32), 0)] },
    Table {
        name: "title",
        per_hundred: 100,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("title", Col::Str(200), 0),
            ("imdb_index", Col::Str(12), 90),
            ("kind_id", Col::Ref("kind_type"), 0),
            ("production_year", Col::Year, 5),
            ("imdb_id", Col::Int(1_000_000), 50),
            ("phonetic_code", Col::Str(5), 10),
            ("episode_of_id", Col::Ref("title"), 70),
            ("season_nr", Col::Int(30), 70),
            ("episode_nr", Col::Int(200), 70),
            ("series_years", Col::Str(49), 90),
            ("md5sum", Col::Str(32), 0),
        ],
    },
    Table {
        name: "keyword",
        per_hundred: 40,
        fixed: false,
        columns: &[("id", Col::Id, 0), ("keyword", Col::Str(200), 0), ("phonetic_code", Col::Str(5), 5)],
    },
    Table {
        name: "company_name",
        per_hundred: 10,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("name", Col::Str(200), 0),
            ("country_code", Col::Str(255), 30),
            ("imdb_id", Col::Int(1_000_000), 90),
            ("name_pcode_nf", Col::Str(5), 10),
            ("name_pcode_sf", Col::Str(5), 40),
            ("md5sum", Col::Str(32), 0),
        ],
    },
    Table {
        name: "name",
        per_hundred: 150,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("name", Col::Str(200), 0),
            ("imdb_index", Col::Str(12), 90),
            ("imdb_id", Col::Int(1_000_000), 90),
            ("gender", Col::Str(1), 40),
            ("name_pcode_cf", Col::Str(5), 5),
            ("name_pcode_nf", Col::Str(5), 5),
            ("surname_pcode", Col::Str(5), 20),
            ("md5sum", Col::Str(32), 0),
        ],
    },
    Table {
        name: "char_name",
        per_hundred: 100,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("name", Col::Str(200), 0),
            ("imdb_index", Col::Str(12), 95),
            ("imdb_id", Col::Int(1_000_000), 95),
            ("name_pcode_nf", Col::Str(5), 5),
            ("surname_pcode", Col::Str(5), 30),
            ("md5sum", Col::Str(32), 0),
        ],
    },
    Table {
        name: "aka_name",
        per_hundred: 30,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("person_id", Col::Ref("name"), 0),
            ("name", Col::Str(200), 0),
            ("imdb_index", Col::Str(12), 90),
            ("name_pcode_cf", Col::Str(5), 5),
            ("name_pcode_nf", Col::Str(5), 5),
            ("surname_pcode", Col::Str(5), 20),
            ("md5sum", Col::Str(32), 0),
        ],
    },
    Table {
        name: "aka_title",
        per_hundred: 15,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("movie_id", Col::Ref("title"), 0),
            ("title", Col::Str(200), 0),
            ("imdb_index", Col::Str(12), 90),
            ("kind_id", Col::Ref("kind_type"), 0),
            ("production_year", Col::Year, 5),
            ("phonetic_code", Col::Str(5), 10),
            ("episode_of_id", Col::Ref("title"), 80),
            ("season_nr", Col::Int(30), 80),
            ("episode_nr", Col::Int(200), 80),
            ("note", Col::Str(200), 50),
            ("md5sum", Col::Str(32), 0),
        ],
    },
    Table {
        name: "cast_info",
        per_hundred: 800,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("person_id", Col::Ref("name"), 0),
            ("movie_id", Col::Ref("title"), 0),
            ("person_role_id", Col::Ref("char_name"), 40),
            ("note", Col::Str(200), 50),
            ("nr_order", Col::Int(100), 40),
            ("role_id", Col::Ref("role_type"), 0),
        ],
    },
    Table {
        name: "complete_cast",
        per_hundred: 10,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("movie_id", Col::Ref("title"), 0),
            ("subject_id", Col::Ref("comp_cast_type"), 0),
            ("status_id", Col::Ref("comp_cast_type"), 0),
        ],
    },
    Table {
        name: "movie_companies",
        per_hundred: 120,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("movie_id", Col::Ref("title"), 0),
            ("company_id", Col::Ref("company_name"), 0),
            ("company_type_id", Col::Ref("company_type"), 0),
            ("note", Col::Str(200), 40),
        ],
    },
    Table {
        name: "movie_info",
        per_hundred: 500,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("movie_id", Col::Ref("title"), 0),
            ("info_type_id", Col::Ref("info_type"), 0),
            ("info", Col::Str(200), 0),
            ("note", Col::Str(200), 60),
        ],
    },
    Table {
        name: "movie_info_idx",
        per_hundred: 60,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("movie_id", Col::Ref("title"), 0),
            ("info_type_id", Col::Ref("info_type"), 0),
            ("info", Col::Str(200), 0),
            ("note", Col::Str(200), 90),
        ],
    },
    Table {
        name: "movie_keyword",
        per_hundred: 200,
        fixed: false,
        columns: &[("id", Col::Id, 0), ("movie_id", Col::Ref("title"), 0), ("keyword_id", Col::Ref("keyword"), 0)],
    },
    Table {
        name: "movie_link",
        per_hundred: 5,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("movie_id", Col::Ref("title"), 0),
            ("linked_movie_id", Col::Ref("title"), 0),
            ("link_type_id", Col::Ref("link_type"), 0),
        ],
    },
    Table {
        name: "person_info",
        per_hundred: 100,
        fixed: false,
        columns: &[
            ("id", Col::Id, 0),
            ("person_id", Col::Ref("name"), 0),
            ("info_type_id", Col::Ref("info_type"), 0),
            ("info", Col::Str(200), 0),
            ("note", Col::Str(200), 70),
        ],
    },
];

/// The percent of a string column's values taken from the queries'.
const WORDS: u64 = 85;
/// The rows of one INSERT statement.
const BATCH: u64 = 500;
/// The seed the data is made from.
const SEED: u64 = 0x1DB0_2015;

/// The queries, each its name and its text.
pub fn queries() -> Vec<(&'static str, &'static str)> {
    QUERIES
        .split("-- ")
        .filter_map(|part| part.split_once('\n'))
        .map(|(name, sql)| (name.trim(), sql.trim().trim_end_matches(';')))
        .collect()
}

/// xorshift64*, from a fixed seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12_u32;
        self.0 ^= self.0 << 25_u32;
        self.0 ^= self.0 >> 27_u32;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A number in `0..n`, of none `0`.
    fn below(&mut self, n: u64) -> u64 {
        self.next().checked_rem(n).unwrap_or(0)
    }

    /// A number in `0..n` skewed toward 0: the least of three.
    fn skewed(&mut self, n: u64) -> u64 {
        self.below(n).min(self.below(n)).min(self.below(n))
    }

    /// True `percent` times in a hundred.
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// The values the queries compare each `table.column` with: of `=`, IN,
/// LIKE (its wildcards made plain), < and >.
fn vocabulary() -> HashMap<String, Vec<String>> {
    let mut found: HashMap<String, Vec<String>> = HashMap::new();
    for (_, sql) in queries() {
        let aliases = aliases(sql);
        let tokens = tokens(sql);
        for (at, token) in tokens.iter().enumerate() {
            let Some((alias, column)) = token.split_once('.') else { continue };
            let Some(table) = aliases.get(alias) else { continue };
            let values = found.entry(format!("{table}.{column}")).or_default();
            let compared =
                tokens.iter().skip(at + 1).take(64).take_while(|t| (t.starts_with('\'') || !t.contains('.')) && *t != "AND" && *t != "OR");
            for plain in compared.filter_map(|value| literal(value)) {
                if !values.contains(&plain) {
                    values.push(plain);
                }
            }
        }
    }
    found
}

/// The text of the quoted literal `token`, its wildcards made plain; none
/// if it is not one or holds nothing.
fn literal(token: &str) -> Option<String> {
    let text = token.strip_prefix('\'')?;
    let plain = text.trim_end_matches('\'').replace("''", "'").replace('%', " ").replace('_', "x");
    let plain = plain.trim().to_owned();
    (!plain.is_empty()).then_some(plain)
}

/// The tables of `sql`'s FROM clause by their aliases.
fn aliases(sql: &str) -> HashMap<String, String> {
    let tokens = tokens(sql);
    let mut out = HashMap::new();
    for (at, token) in tokens.iter().enumerate() {
        if token == "AS"
            && let (Some(table), Some(alias)) = (at.checked_sub(1).and_then(|b| tokens.get(b)), tokens.get(at + 1))
            && TABLES.iter().any(|t| t.name == table)
        {
            out.insert(alias.clone(), table.clone());
        }
    }
    out
}

/// The words, quoted strings and signs of `sql`.
fn tokens(sql: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = sql.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\'' {
            let mut text = String::from('\'');
            while let Some(d) = chars.next() {
                text.push(d);
                if d == '\'' && chars.peek() != Some(&'\'') {
                    break;
                }
                if d == '\'' {
                    chars.next();
                    text.push('\'');
                }
            }
            out.push(text);
        } else if c.is_alphanumeric() || c == '_' || c == '.' {
            let mut word = String::from(c);
            while let Some(&d) = chars.peek().filter(|d| d.is_alphanumeric() || **d == '_' || **d == '.') {
                word.push(d);
                chars.next();
            }
            out.push(word);
        } else if !c.is_whitespace() {
            out.push(c.to_string());
        }
    }
    out
}

/// Makes the database of `titles` titles: its schema, its rows, its
/// indexes and its statistics.
pub fn load(titles: u64) -> Database {
    let db = Database::new();
    db.schema(SCHEMA).unwrap();
    let s = db.session("IMDB");
    let vocabulary = vocabulary();
    let mut rng = Rng(SEED);
    let mut counts: HashMap<&str, u64> = HashMap::new();
    let mut themes: Vec<u64> = Vec::new();
    for table in &TABLES {
        let rows = if table.fixed { table.per_hundred } else { (titles * table.per_hundred).div_ceil(100).max(1) };
        counts.insert(table.name, rows);
        let names: Vec<&str> = table.columns.iter().map(|c| c.0).collect();
        let mut values = Vec::new();
        for id in 1..=rows {
            let theme = if table.name == "title" {
                let theme = rng.skewed(64);
                themes.push(theme);
                theme
            } else {
                id
            };
            values.push(row(table, (id, theme), (&mut rng, &vocabulary, &counts, &themes)));
            if values.len() as u64 == BATCH || id == rows {
                s.run(&format!("INSERT INTO {} ({}) VALUES {}", table.name, names.join(", "), values.join(", "))).unwrap();
                values.clear();
            }
        }
        s.commit().unwrap();
    }
    for (table, column) in INDEXES {
        let (table, column) = (table.to_uppercase(), column.to_uppercase());
        db.create_index(&format!("{column}_{table}"), &format!("IMDB.{table}"), &[&column], "btree").unwrap();
    }
    db.maintain().unwrap();
    db
}

/// What a row is made from: the generator, the queries' values, the rows
/// of each table made so far, and each title's theme.
type Made<'a> = (&'a mut Rng, &'a HashMap<String, Vec<String>>, &'a HashMap<&'a str, u64>, &'a [u64]);

/// A row of `table`, the `id`th, of the theme `theme`, as SQL text. A row
/// of a movie takes its theme.
fn row(table: &Table, (id, mut theme): (u64, u64), (rng, vocabulary, counts, themes): Made<'_>) -> String {
    let mut out = Vec::new();
    for &(column, col, nulls) in table.columns {
        if nulls > 0 && rng.chance(nulls) {
            out.push("NULL".to_owned());
            continue;
        }
        let value = match col {
            Col::Id => id.to_string(),
            Col::Ref(of) => {
                let rows = counts.get(of).copied().unwrap_or(1).max(1);
                let picked = if rows <= 113 { (theme % 8 + rng.skewed(rows)) % rows } else { rng.skewed(rows) };
                if column == "movie_id" {
                    theme = themes.get(picked as usize).copied().unwrap_or(theme);
                }
                (picked + 1).to_string()
            }
            Col::Year => (1950 + (theme * 7 + rng.below(25)) % 70).to_string(),
            Col::Int(most) => rng.below(most).to_string(),
            Col::Str(length) => {
                let words = vocabulary.get(&format!("{}.{column}", table.name)).map_or(&[][..], Vec::as_slice);
                let fits: Vec<&String> = words.iter().filter(|w| w.len() <= length).collect();
                let text = string((&fits, length), (id, theme), rng, table.fixed);
                format!("'{}'", text.replace('\'', "''"))
            }
        };
        out.push(value);
    }
    format!("({})", out.join(", "))
}

/// A string of at most `length` characters: of a table of kinds, its
/// `id`th of `words`, else made up; else, `WORDS` times in a hundred, one of `words`
/// near the theme's, padded as a LIKE pattern finds it, else made up.
fn string((words, length): (&[&String], usize), (id, theme): (u64, u64), rng: &mut Rng, fixed: bool) -> String {
    let made = format!("v{}x{}", theme, rng.below(1000));
    let count = words.len() as u64;
    let chosen = if fixed {
        words.get((id as usize).saturating_sub(1)).map(|w| (*w).clone())
    } else if count > 0 && rng.chance(WORDS) {
        words.get(((theme + rng.skewed(count)) % count) as usize).map(|w| (*w).clone())
    } else {
        None
    };
    let text = chosen.unwrap_or(made);
    text.chars().take(length).collect()
}
