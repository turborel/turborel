//! The web log's table (`weblog.rs`). HITS has 105 columns and holds the
//! page views of a web analytics service, one row a view. Whole numbers
//! of 64 bits are NUMERIC(19, 0), since identifiers and hashes fill them.
//! Texts are under BYTES, a collation of octets with no padding, so two
//! texts are equal only if their octets are. The rows are kept in the
//! order of the counter, the date, the user, the time and the view.

/// What a column holds.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// SMALLINT.
    Small,
    /// INTEGER.
    Int,
    /// NUMERIC(19, 0).
    Big,
    /// VARCHAR(32767) under the collation of octets.
    Text,
    /// CHARACTER(1).
    Flag,
    /// TIMESTAMP.
    Time,
    /// DATE.
    Date,
}

impl Kind {
    /// Its type, as the table's definition writes it.
    fn sql(self) -> &'static str {
        match self {
            Kind::Small => "SMALLINT",
            Kind::Int => "INTEGER",
            Kind::Big => "NUMERIC(19, 0)",
            Kind::Text => "VARCHAR(32767) COLLATE B.BYTES",
            Kind::Flag => "CHARACTER(1)",
            Kind::Time => "TIMESTAMP",
            Kind::Date => "DATE",
        }
    }
}

/// How many columns HITS has.
pub const WIDTH: usize = 105;

/// The columns of HITS, in order, each with what it holds.
pub const COLUMNS: [(&str, Kind); WIDTH] = [
    ("WatchID", Kind::Big),
    ("JavaEnable", Kind::Small),
    ("Title", Kind::Text),
    ("GoodEvent", Kind::Small),
    ("EventTime", Kind::Time),
    ("EventDate", Kind::Date),
    ("CounterID", Kind::Int),
    ("ClientIP", Kind::Int),
    ("RegionID", Kind::Int),
    ("UserID", Kind::Big),
    ("CounterClass", Kind::Small),
    ("OS", Kind::Small),
    ("UserAgent", Kind::Small),
    ("URL", Kind::Text),
    ("Referer", Kind::Text),
    ("IsRefresh", Kind::Small),
    ("RefererCategoryID", Kind::Small),
    ("RefererRegionID", Kind::Int),
    ("URLCategoryID", Kind::Small),
    ("URLRegionID", Kind::Int),
    ("ResolutionWidth", Kind::Small),
    ("ResolutionHeight", Kind::Small),
    ("ResolutionDepth", Kind::Small),
    ("FlashMajor", Kind::Small),
    ("FlashMinor", Kind::Small),
    ("FlashMinor2", Kind::Text),
    ("NetMajor", Kind::Small),
    ("NetMinor", Kind::Small),
    ("UserAgentMajor", Kind::Small),
    ("UserAgentMinor", Kind::Text),
    ("CookieEnable", Kind::Small),
    ("JavascriptEnable", Kind::Small),
    ("IsMobile", Kind::Small),
    ("MobilePhone", Kind::Small),
    ("MobilePhoneModel", Kind::Text),
    ("Params", Kind::Text),
    ("IPNetworkID", Kind::Int),
    ("TraficSourceID", Kind::Small),
    ("SearchEngineID", Kind::Small),
    ("SearchPhrase", Kind::Text),
    ("AdvEngineID", Kind::Small),
    ("IsArtifical", Kind::Small),
    ("WindowClientWidth", Kind::Small),
    ("WindowClientHeight", Kind::Small),
    ("ClientTimeZone", Kind::Small),
    ("ClientEventTime", Kind::Time),
    ("SilverlightVersion1", Kind::Small),
    ("SilverlightVersion2", Kind::Small),
    ("SilverlightVersion3", Kind::Int),
    ("SilverlightVersion4", Kind::Small),
    ("PageCharset", Kind::Text),
    ("CodeVersion", Kind::Int),
    ("IsLink", Kind::Small),
    ("IsDownload", Kind::Small),
    ("IsNotBounce", Kind::Small),
    ("FUniqID", Kind::Big),
    ("OriginalURL", Kind::Text),
    ("HID", Kind::Int),
    ("IsOldCounter", Kind::Small),
    ("IsEvent", Kind::Small),
    ("IsParameter", Kind::Small),
    ("DontCountHits", Kind::Small),
    ("WithHash", Kind::Small),
    ("HitColor", Kind::Flag),
    ("LocalEventTime", Kind::Time),
    ("Age", Kind::Small),
    ("Sex", Kind::Small),
    ("Income", Kind::Small),
    ("Interests", Kind::Small),
    ("Robotness", Kind::Small),
    ("RemoteIP", Kind::Int),
    ("WindowName", Kind::Int),
    ("OpenerName", Kind::Int),
    ("HistoryLength", Kind::Small),
    ("BrowserLanguage", Kind::Text),
    ("BrowserCountry", Kind::Text),
    ("SocialNetwork", Kind::Text),
    ("SocialAction", Kind::Text),
    ("HTTPError", Kind::Small),
    ("SendTiming", Kind::Int),
    ("DNSTiming", Kind::Int),
    ("ConnectTiming", Kind::Int),
    ("ResponseStartTiming", Kind::Int),
    ("ResponseEndTiming", Kind::Int),
    ("FetchTiming", Kind::Int),
    ("SocialSourceNetworkID", Kind::Small),
    ("SocialSourcePage", Kind::Text),
    ("ParamPrice", Kind::Big),
    ("ParamOrderID", Kind::Text),
    ("ParamCurrency", Kind::Text),
    ("ParamCurrencyID", Kind::Small),
    ("OpenstatServiceName", Kind::Text),
    ("OpenstatCampaignID", Kind::Text),
    ("OpenstatAdID", Kind::Text),
    ("OpenstatSourceID", Kind::Text),
    ("UTMSource", Kind::Text),
    ("UTMMedium", Kind::Text),
    ("UTMCampaign", Kind::Text),
    ("UTMContent", Kind::Text),
    ("UTMTerm", Kind::Text),
    ("FromTag", Kind::Text),
    ("HasGCLID", Kind::Small),
    ("RefererHash", Kind::Big),
    ("URLHash", Kind::Big),
    ("CLID", Kind::Int),
];

/// The collation of octets the texts are under.
const COLLATION: &str = "CREATE COLLATION BYTES FOR SQL_TEXT FROM UCS_BASIC NO PAD";
/// The order HITS keeps its rows in, and how a load writes them, each
/// batch straight as a frozen group.
const OPTIONS: &str =
    "USING columnstore WITH (COLUMNSTORE.ORDER = 'CounterID, EventDate, UserID, EventTime, WatchID', COLUMNSTORE.LOAD = 'GROUPS')";

/// The statements that define schema B's collation and HITS.
pub fn definitions() -> [String; 2] {
    let columns: Vec<String> = COLUMNS.iter().map(|(name, kind)| format!("{name} {}", kind.sql())).collect();
    [COLLATION.to_owned(), format!("CREATE TABLE HITS ({}) {OPTIONS}", columns.join(", "))]
}

/// The INSERT a load prepares, of one row, a parameter a column.
pub fn insert() -> String {
    format!("INSERT INTO HITS VALUES ({})", vec!["?"; WIDTH].join(", "))
}
