//! The web log's 43 queries (`weblog.rs`), in standard SQL: counts and
//! sums of the whole table, groupings by keys of few and of many values,
//! DISTINCT set functions, searches of texts by LIKE and by a regular
//! expression, the first rows by a sort, and a counter's views of a month
//! grouped. The constants that depend on the rows made stand as names in
//! braces, which `text` replaces: a user and two hashes that exist, the
//! views a group needs to be kept, and the rows skipped before the first
//! returned, each a share of the rows as the original's was of its own.

/// The sums of the query of ninety, each of the width and a constant.
const SUMS: u32 = 90;
/// The query of ninety sums' place among `QUERIES`.
const NINETY: usize = 29;

/// The queries, each with the name it is reported and chosen by.
const QUERIES: [(&str, &str); 43] = [
    ("q01", "SELECT COUNT(*) FROM hits"),
    ("q02", "SELECT COUNT(*) FROM hits WHERE AdvEngineID <> 0"),
    ("q03", "SELECT SUM(AdvEngineID), COUNT(*), AVG(ResolutionWidth) FROM hits"),
    ("q04", "SELECT AVG(UserID) FROM hits"),
    ("q05", "SELECT COUNT(DISTINCT UserID) FROM hits"),
    ("q06", "SELECT COUNT(DISTINCT SearchPhrase) FROM hits"),
    ("q07", "SELECT MIN(EventDate), MAX(EventDate) FROM hits"),
    ("q08", "SELECT AdvEngineID, COUNT(*) AS c FROM hits WHERE AdvEngineID <> 0 GROUP BY AdvEngineID ORDER BY c DESC"),
    ("q09", "SELECT RegionID, COUNT(DISTINCT UserID) AS u FROM hits GROUP BY RegionID ORDER BY u DESC FETCH FIRST 10 ROWS ONLY"),
    (
        "q10",
        "SELECT RegionID, SUM(AdvEngineID), COUNT(*) AS c, AVG(ResolutionWidth), COUNT(DISTINCT UserID) FROM hits GROUP BY RegionID \
         ORDER BY c DESC FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q11",
        "SELECT MobilePhoneModel, COUNT(DISTINCT UserID) AS u FROM hits WHERE MobilePhoneModel <> '' GROUP BY MobilePhoneModel ORDER \
         BY u DESC FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q12",
        "SELECT MobilePhone, MobilePhoneModel, COUNT(DISTINCT UserID) AS u FROM hits WHERE MobilePhoneModel <> '' GROUP BY \
         MobilePhone, MobilePhoneModel ORDER BY u DESC FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q13",
        "SELECT SearchPhrase, COUNT(*) AS c FROM hits WHERE SearchPhrase <> '' GROUP BY SearchPhrase ORDER BY c DESC FETCH FIRST 10 \
         ROWS ONLY",
    ),
    (
        "q14",
        "SELECT SearchPhrase, COUNT(DISTINCT UserID) AS u FROM hits WHERE SearchPhrase <> '' GROUP BY SearchPhrase ORDER BY u DESC \
         FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q15",
        "SELECT SearchEngineID, SearchPhrase, COUNT(*) AS c FROM hits WHERE SearchPhrase <> '' GROUP BY SearchEngineID, SearchPhrase \
         ORDER BY c DESC FETCH FIRST 10 ROWS ONLY",
    ),
    ("q16", "SELECT UserID, COUNT(*) AS c FROM hits GROUP BY UserID ORDER BY c DESC FETCH FIRST 10 ROWS ONLY"),
    ("q17", "SELECT UserID, SearchPhrase, COUNT(*) AS c FROM hits GROUP BY UserID, SearchPhrase ORDER BY c DESC FETCH FIRST 10 ROWS ONLY"),
    ("q18", "SELECT UserID, SearchPhrase, COUNT(*) FROM hits GROUP BY UserID, SearchPhrase FETCH FIRST 10 ROWS ONLY"),
    (
        "q19",
        "SELECT UserID, m, SearchPhrase, COUNT(*) AS c FROM (SELECT UserID, EXTRACT(MINUTE FROM EventTime) AS m, SearchPhrase FROM \
         hits) AS t GROUP BY UserID, m, SearchPhrase ORDER BY c DESC FETCH FIRST 10 ROWS ONLY",
    ),
    ("q20", "SELECT UserID FROM hits WHERE UserID = {user}"),
    ("q21", "SELECT COUNT(*) FROM hits WHERE URL LIKE '%google%'"),
    (
        "q22",
        "SELECT SearchPhrase, MIN(URL), COUNT(*) AS c FROM hits WHERE URL LIKE '%google%' AND SearchPhrase <> '' GROUP BY SearchPhrase \
         ORDER BY c DESC FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q23",
        "SELECT SearchPhrase, MIN(URL), MIN(Title), COUNT(*) AS c, COUNT(DISTINCT UserID) FROM hits WHERE Title LIKE '%Google%' AND \
         URL NOT LIKE '%.google.%' AND SearchPhrase <> '' GROUP BY SearchPhrase ORDER BY c DESC FETCH FIRST 10 ROWS ONLY",
    ),
    ("q24", "SELECT * FROM hits WHERE URL LIKE '%google%' ORDER BY EventTime FETCH FIRST 10 ROWS ONLY"),
    ("q25", "SELECT SearchPhrase FROM hits WHERE SearchPhrase <> '' ORDER BY EventTime FETCH FIRST 10 ROWS ONLY"),
    ("q26", "SELECT SearchPhrase FROM hits WHERE SearchPhrase <> '' ORDER BY SearchPhrase FETCH FIRST 10 ROWS ONLY"),
    ("q27", "SELECT SearchPhrase FROM hits WHERE SearchPhrase <> '' ORDER BY EventTime, SearchPhrase FETCH FIRST 10 ROWS ONLY"),
    (
        "q28",
        "SELECT CounterID, AVG(OCTET_LENGTH(URL)) AS l, COUNT(*) AS c FROM hits WHERE URL <> '' GROUP BY CounterID HAVING COUNT(*) > \
         {having} ORDER BY l DESC FETCH FIRST 25 ROWS ONLY",
    ),
    (
        "q29",
        "SELECT k, AVG(OCTET_LENGTH(Referer)) AS l, COUNT(*) AS c, MIN(Referer) FROM (SELECT \
         TRANSLATE_REGEX('^https?://(www\\.)?([^/]+)/.*$' FLAG 's' IN Referer WITH '$2') AS k, Referer FROM hits WHERE Referer <> '') \
         AS t GROUP BY k HAVING COUNT(*) > {having} ORDER BY l DESC FETCH FIRST 25 ROWS ONLY",
    ),
    ("q30", ""),
    (
        "q31",
        "SELECT SearchEngineID, ClientIP, COUNT(*) AS c, SUM(IsRefresh), AVG(ResolutionWidth) FROM hits WHERE SearchPhrase <> '' GROUP \
         BY SearchEngineID, ClientIP ORDER BY c DESC FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q32",
        "SELECT WatchID, ClientIP, COUNT(*) AS c, SUM(IsRefresh), AVG(ResolutionWidth) FROM hits WHERE SearchPhrase <> '' GROUP BY \
         WatchID, ClientIP ORDER BY c DESC FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q33",
        "SELECT WatchID, ClientIP, COUNT(*) AS c, SUM(IsRefresh), AVG(ResolutionWidth) FROM hits GROUP BY WatchID, ClientIP ORDER BY c \
         DESC FETCH FIRST 10 ROWS ONLY",
    ),
    ("q34", "SELECT URL, COUNT(*) AS c FROM hits GROUP BY URL ORDER BY c DESC FETCH FIRST 10 ROWS ONLY"),
    ("q35", "SELECT 1, URL, COUNT(*) AS c FROM hits GROUP BY URL ORDER BY c DESC FETCH FIRST 10 ROWS ONLY"),
    (
        "q36",
        "SELECT ClientIP, c1, c2, c3, COUNT(*) AS c FROM (SELECT ClientIP, ClientIP - 1 AS c1, ClientIP - 2 AS c2, ClientIP - 3 AS c3 \
         FROM hits) AS t GROUP BY ClientIP, c1, c2, c3 ORDER BY c DESC FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q37",
        "SELECT URL, COUNT(*) AS PageViews FROM hits WHERE CounterID = 62 AND EventDate >= DATE '2013-07-01' AND EventDate <= DATE \
         '2013-07-31' AND DontCountHits = 0 AND IsRefresh = 0 AND URL <> '' GROUP BY URL ORDER BY PageViews DESC FETCH FIRST 10 ROWS \
         ONLY",
    ),
    (
        "q38",
        "SELECT Title, COUNT(*) AS PageViews FROM hits WHERE CounterID = 62 AND EventDate >= DATE '2013-07-01' AND EventDate <= DATE \
         '2013-07-31' AND DontCountHits = 0 AND IsRefresh = 0 AND Title <> '' GROUP BY Title ORDER BY PageViews DESC FETCH FIRST 10 \
         ROWS ONLY",
    ),
    (
        "q39",
        "SELECT URL, COUNT(*) AS PageViews FROM hits WHERE CounterID = 62 AND EventDate >= DATE '2013-07-01' AND EventDate <= DATE \
         '2013-07-31' AND IsRefresh = 0 AND IsLink <> 0 AND IsDownload = 0 GROUP BY URL ORDER BY PageViews DESC OFFSET {skip_more} \
         ROWS FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q40",
        "SELECT TraficSourceID, SearchEngineID, AdvEngineID, Src, Dst, COUNT(*) AS PageViews FROM (SELECT TraficSourceID, \
         SearchEngineID, AdvEngineID, CASE WHEN (SearchEngineID = 0 AND AdvEngineID = 0) THEN Referer ELSE '' END AS Src, URL AS Dst \
         FROM hits WHERE CounterID = 62 AND EventDate >= DATE '2013-07-01' AND EventDate <= DATE '2013-07-31' AND IsRefresh = 0) AS t \
         GROUP BY TraficSourceID, SearchEngineID, AdvEngineID, Src, Dst ORDER BY PageViews DESC OFFSET {skip_more} ROWS FETCH FIRST 10 \
         ROWS ONLY",
    ),
    (
        "q41",
        "SELECT URLHash, EventDate, COUNT(*) AS PageViews FROM hits WHERE CounterID = 62 AND EventDate >= DATE '2013-07-01' AND \
         EventDate <= DATE '2013-07-31' AND IsRefresh = 0 AND TraficSourceID IN (-1, 6) AND RefererHash = {referer} GROUP BY URLHash, \
         EventDate ORDER BY PageViews DESC OFFSET {skip} ROWS FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q42",
        "SELECT WindowClientWidth, WindowClientHeight, COUNT(*) AS PageViews FROM hits WHERE CounterID = 62 AND EventDate >= DATE \
         '2013-07-01' AND EventDate <= DATE '2013-07-31' AND IsRefresh = 0 AND DontCountHits = 0 AND URLHash = {url} GROUP BY \
         WindowClientWidth, WindowClientHeight ORDER BY PageViews DESC OFFSET {skip_most} ROWS FETCH FIRST 10 ROWS ONLY",
    ),
    (
        "q43",
        "SELECT M, COUNT(*) AS PageViews FROM (SELECT EventTime - EXTRACT(SECOND FROM EventTime) * INTERVAL '1' SECOND AS M FROM hits \
         WHERE CounterID = 62 AND EventDate >= DATE '2013-07-14' AND EventDate <= DATE '2013-07-15' AND IsRefresh = 0 AND \
         DontCountHits = 0) AS t GROUP BY M ORDER BY M OFFSET {skip_more} ROWS FETCH FIRST 10 ROWS ONLY",
    ),
];

/// What the texts' names in braces stand for, of one table's rows.
pub struct Constants {
    /// A user of a few views.
    pub user: i64,
    /// The hash of a referer of counter 62, of many views.
    pub referer: i64,
    /// The hash of counter 62's page of most views.
    pub url: i64,
    /// The views a group has more than, to be kept.
    pub having: u64,
    /// The rows skipped of the groupings of counter 62: of its views by
    /// one referer, of its pages, and of the sizes of windows on one page.
    pub skips: [u64; 3],
}

/// How many queries there are.
pub fn count() -> usize {
    QUERIES.len()
}

/// The name of query `n`.
pub fn name(n: usize) -> &'static str {
    QUERIES[n].0
}

/// The text of query `n`, its constants those of `constants`.
pub fn text(n: usize, constants: &Constants) -> String {
    if n == NINETY {
        let sums: Vec<String> = (1..SUMS).map(|k| format!("SUM(ResolutionWidth + {k})")).collect();
        return format!("SELECT SUM(ResolutionWidth), {} FROM hits", sums.join(", "));
    }
    let [skip, skip_more, skip_most] = constants.skips;
    QUERIES[n]
        .1
        .replace("{user}", &constants.user.to_string())
        .replace("{referer}", &constants.referer.to_string())
        .replace("{url}", &constants.url.to_string())
        .replace("{having}", &constants.having.to_string())
        .replace("{skip_most}", &skip_most.to_string())
        .replace("{skip_more}", &skip_more.to_string())
        .replace("{skip}", &skip.to_string())
}
