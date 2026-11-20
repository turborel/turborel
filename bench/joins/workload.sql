-- w001
SELECT MIN(t.production_year) AS t_production_year,
       MIN(t.title) AS t_title,
       MIN(mx.info) AS mx_info
FROM movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     title AS t
WHERE mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND mx.movie_id = mk.movie_id
  AND mx.info > '4.5'
  AND t.title LIKE '%River%';

-- w002
SELECT MIN(lnk.title) AS lnk_title
FROM aka_title AS akt,
     complete_cast AS cc,
     company_name AS cn,
     kind_type AS lkt,
     title AS lnk,
     movie_companies AS mc,
     movie_info AS mi,
     info_type AS mit,
     movie_link AS ml,
     movie_info_idx AS mx,
     title AS t
WHERE akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mx.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.kind_id = lkt.id
  AND cc.movie_id = mc.movie_id
  AND t.production_year > 1991
  AND lkt.kind = 'anthology'
  AND mx.info > '6.0'
  AND mit.info = 'tagline'
  AND mi.info IN ('Thriller', 'Musical');

-- w003
SELECT MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year
FROM cast_info AS ci,
     movie_keyword AS mk,
     role_type AS rt,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND mk.movie_id = t.id
  AND mk.movie_id = ci.movie_id
  AND rt.role IN ('extra', 'composer', 'stunt')
  AND t.production_year > 1998;

-- w004
SELECT MIN(t.title) AS t_title
FROM aka_title AS akt,
     aka_name AS an,
     char_name AS chn,
     cast_info AS ci,
     keyword AS k,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     name AS n,
     role_type AS rt,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.person_id = n.id
  AND ci.person_role_id = chn.id
  AND ci.role_id = rt.id
  AND an.person_id = n.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND mi.info IN ('Musical', 'Satire', 'Heist')
  AND chn.name LIKE '%Mayor%'
  AND n.name LIKE '%Kai%'
  AND ci.note = '(cameo)'
  AND mx.info > '7.5';

-- w005
SELECT MIN(t.production_year) AS t_production_year,
       MIN(lnk.title) AS lnk_title,
       MIN(t.title) AS t_title
FROM kind_type AS lkt,
     title AS lnk,
     movie_link AS ml,
     title AS t
WHERE ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.kind_id = lkt.id
  AND lkt.kind IN ('anthology', 'serial', 'feature')
  AND t.production_year BETWEEN 1992 AND 1995
  AND lnk.production_year < 2000;

-- w006
SELECT MIN(t.title) AS t_title,
       MIN(pi.info) AS pi_info,
       MIN(mc.note) AS mc_note
FROM aka_title AS akt,
     char_name AS chn,
     cast_info AS ci,
     company_name AS cn,
     company_type AS ct,
     movie_companies AS mc,
     movie_info_idx AS mx,
     info_type AS mxt,
     name AS n,
     person_info AS pi,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.person_id = n.id
  AND ci.person_role_id = chn.id
  AND pi.person_id = n.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mc.movie_id = akt.movie_id
  AND n.gender = 'f'
  AND t.title LIKE '%Echo%'
  AND akt.title LIKE '%Harbor%';

-- w007
SELECT MIN(t.production_year) AS t_production_year,
       MIN(mx.info) AS mx_info
FROM aka_title AS akt,
     kind_type AS kt,
     movie_info_idx AS mx,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND mx.movie_id = t.id
  AND akt.movie_id = mx.movie_id
  AND t.title LIKE '%Echo%'
  AND kt.kind IN ('special', 'feature')
  AND mx.info > '7.5'
  AND akt.title LIKE '%Echo%';

-- w008
SELECT MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     kind_type AS lkt,
     title AS lnk,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.kind_id = lkt.id
  AND lnk.production_year < 1985
  AND ci.note IN ('(cameo)', '(singing)')
  AND lkt.kind IN ('feature', 'anthology', 'special');

-- w009
SELECT MIN(t.production_year) AS t_production_year,
       MIN(t.title) AS t_title,
       MIN(chn.name) AS chn_name
FROM char_name AS chn,
     cast_info AS ci,
     movie_link AS ml,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND ml.movie_id = t.id
  AND t.production_year BETWEEN 1961 AND 1971
  AND chn.name LIKE '%Pilot%'
  AND ci.note = '(cameo)';

-- w010
SELECT MIN(lt.link) AS lt_link
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     keyword AS k,
     title AS lnk,
     link_type AS lt,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND cc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND lt.link IN ('spinoff', 'sequel', 'parody')
  AND mi.info IN ('Satire', 'Mystery', 'Heist')
  AND ci.note IN ('(archive)', '(cameo)')
  AND mx.info > '6.0'
  AND k.keyword = 'comet';

-- w011
SELECT MIN(t.title) AS t_title,
       MIN(k.keyword) AS k_keyword,
       MIN(mx.info) AS mx_info
FROM keyword AS k,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     title AS t
WHERE mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND mk.movie_id = mx.movie_id
  AND k.keyword LIKE '%tide%'
  AND mx.info > '8.0'
  AND t.title LIKE '%River%';

-- w012
SELECT MIN(kt.kind) AS kt_kind,
       MIN(t.production_year) AS t_production_year
FROM complete_cast AS cc,
     cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     movie_info AS mi,
     info_type AS mit,
     movie_keyword AS mk,
     name AS n,
     comp_cast_type AS stat,
     comp_cast_type AS subj,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND ci.person_id = n.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND cc.status_id = stat.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND n.name LIKE '%Tomas%'
  AND stat.kind IN ('partial', 'verified')
  AND ci.note IN ('(archive)', '(singing)', '(cameo)')
  AND mit.info IN ('runtime', 'locale', 'genre');

-- w013
SELECT MIN(akt.title) AS akt_title
FROM aka_title AS akt,
     kind_type AS kt,
     movie_info AS mi,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND mi.movie_id = t.id
  AND mi.movie_id = akt.movie_id
  AND mi.info IN ('Fable', 'Mystery')
  AND kt.kind = 'special'
  AND akt.title LIKE '%Echo%';

-- w014
SELECT MIN(mi.info) AS mi_info,
       MIN(kt.kind) AS kt_kind,
       MIN(lt.link) AS lt_link
FROM cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     name AS n,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND ci.person_id = n.id
  AND mc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND kt.kind = 'miniseries'
  AND k.keyword IN ('orchard', 'caravan', 'tide')
  AND lt.link IN ('remake', 'homage')
  AND mc.note LIKE '%(Norway)%';

-- w015
SELECT MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year,
       MIN(akt.title) AS akt_title
FROM aka_title AS akt,
     complete_cast AS cc,
     movie_companies AS mc,
     title AS t
WHERE akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND akt.title LIKE '%Night%'
  AND mc.note LIKE '%(Norway)%'
  AND t.production_year > 1991;

-- w016
SELECT MIN(kt.kind) AS kt_kind
FROM aka_title AS akt,
     company_name AS cn,
     company_type AS ct,
     keyword AS k,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND kt.kind = 'serial'
  AND mxt.info IN ('tally', 'score', 'ranking')
  AND mi.info = 'Mystery'
  AND mx.info > '6.0';

-- w017
SELECT MIN(t.production_year) AS t_production_year
FROM kind_type AS kt,
     movie_companies AS mc,
     movie_keyword AS mk,
     title AS t
WHERE t.kind_id = kt.id
  AND mc.movie_id = t.id
  AND mk.movie_id = t.id
  AND mc.movie_id = mk.movie_id
  AND t.production_year BETWEEN 1998 AND 2003
  AND mc.note IS NULL
  AND kt.kind IN ('miniseries', 'special', 'anthology');

-- w018
SELECT MIN(mi.info) AS mi_info,
       MIN(kt.kind) AS kt_kind,
       MIN(lnk.title) AS lnk_title
FROM char_name AS chn,
     cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     kind_type AS lkt,
     title AS lnk,
     movie_info AS mi,
     info_type AS mit,
     movie_keyword AS mk,
     movie_link AS ml,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.kind_id = lkt.id
  AND lkt.kind = 'pilot'
  AND chn.name LIKE '%Mayor%'
  AND ci.note = '(archive)'
  AND mit.info IN ('locale', 'runtime', 'tagline');

-- w019
SELECT MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year
FROM cast_info AS ci,
     kind_type AS kt,
     movie_companies AS mc,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.movie_id = ci.movie_id
  AND mc.note IS NULL
  AND kt.kind IN ('feature', 'shortform')
  AND t.production_year > 1994
  AND ci.note = '(archive)';

-- w020
SELECT MIN(chn.name) AS chn_name
FROM aka_title AS akt,
     complete_cast AS cc,
     char_name AS chn,
     cast_info AS ci,
     kind_type AS kt,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     comp_cast_type AS stat,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND stat.kind IN ('verified', 'partial')
  AND ci.note IN ('(cameo)', '(archive)', '(singing)');

-- w021
SELECT MIN(t.title) AS t_title,
       MIN(cn.name) AS cn_name
FROM aka_title AS akt,
     company_name AS cn,
     movie_companies AS mc,
     title AS t
WHERE akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND akt.movie_id = mc.movie_id
  AND t.production_year > 1991
  AND cn.name LIKE '%Quill%'
  AND akt.title LIKE '%Night%'
  AND mc.note IS NULL;

-- w022
SELECT MIN(lt.link) AS lt_link,
       MIN(akt.title) AS akt_title,
       MIN(mx.info) AS mx_info
FROM aka_title AS akt,
     keyword AS k,
     kind_type AS kt,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND mc.movie_id = akt.movie_id
  AND t.production_year BETWEEN 1998 AND 2006
  AND lnk.production_year < 1995
  AND mc.note IS NULL
  AND mx.info > '4.5';

-- w023
SELECT MIN(kt.kind) AS kt_kind
FROM kind_type AS kt,
     movie_info AS mi,
     movie_link AS ml,
     title AS t
WHERE t.kind_id = kt.id
  AND mi.movie_id = t.id
  AND ml.movie_id = t.id
  AND mi.info = 'Mystery'
  AND kt.kind IN ('feature', 'special', 'anthology')
  AND t.production_year BETWEEN 1998 AND 2008;

-- w024
SELECT MIN(lt.link) AS lt_link,
       MIN(t.production_year) AS t_production_year,
       MIN(t.title) AS t_title
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     link_type AS lt,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     role_type AS rt,
     comp_cast_type AS stat,
     comp_cast_type AS subj,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND akt.movie_id = cc.movie_id
  AND mc.note IS NULL
  AND lt.link IN ('sequel', 'homage')
  AND t.production_year BETWEEN 1983 AND 1996
  AND ci.note IN ('(singing)', '(cameo)')
  AND akt.title LIKE '%Night%';

-- w025
SELECT MIN(kt.kind) AS kt_kind,
       MIN(mc.note) AS mc_note
FROM aka_title AS akt,
     kind_type AS kt,
     movie_companies AS mc,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.movie_id = akt.movie_id
  AND kt.kind IN ('pilot', 'special')
  AND mc.note LIKE '%(festival)%'
  AND t.production_year BETWEEN 1976 AND 1988
  AND akt.title LIKE '%Night%';

-- w026
SELECT MIN(mc.note) AS mc_note
FROM aka_title AS akt,
     char_name AS chn,
     cast_info AS ci,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     info_type AS mit,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     role_type AS rt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND ci.role_id = rt.id
  AND mc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND t.title LIKE '%Stone%'
  AND kt.kind = 'feature'
  AND akt.title LIKE '%Echo%'
  AND mi.info IN ('Western', 'Mystery', 'Satire');

-- w027
SELECT MIN(akt.title) AS akt_title,
       MIN(mc.note) AS mc_note
FROM aka_title AS akt,
     company_name AS cn,
     movie_companies AS mc,
     title AS t
WHERE akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND akt.movie_id = mc.movie_id
  AND t.title LIKE '%Stone%'
  AND mc.note LIKE '%(theatrical)%'
  AND cn.name LIKE '%Harbor%'
  AND akt.title LIKE '%Night%';

-- w028
SELECT MIN(mc.note) AS mc_note,
       MIN(kt.kind) AS kt_kind
FROM complete_cast AS cc,
     cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     name AS n,
     comp_cast_type AS stat,
     comp_cast_type AS subj,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND ci.person_id = n.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND mx.info > '6.0'
  AND subj.kind = 'ensemble'
  AND t.title LIKE '%Stone%'
  AND k.keyword LIKE '%house%'
  AND kt.kind IN ('feature', 'serial');

-- w029
SELECT MIN(t.title) AS t_title,
       MIN(mc.note) AS mc_note
FROM aka_title AS akt,
     complete_cast AS cc,
     movie_companies AS mc,
     title AS t
WHERE akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND akt.title LIKE '%Night%'
  AND mc.note LIKE '%(theatrical)%';

-- w030
SELECT MIN(mx.info) AS mx_info,
       MIN(t.title) AS t_title,
       MIN(cn.name) AS cn_name
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     company_name AS cn,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info_idx AS mx,
     info_type AS mxt,
     comp_cast_type AS stat,
     comp_cast_type AS subj,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mx.movie_id = akt.movie_id
  AND kt.kind IN ('feature', 'serial')
  AND cn.country_code = '[ca]';

-- w031
SELECT MIN(t.title) AS t_title,
       MIN(mx.info) AS mx_info,
       MIN(akt.title) AS akt_title
FROM aka_title AS akt,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     title AS t
WHERE akt.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND akt.movie_id = mx.movie_id
  AND akt.title LIKE '%Echo%'
  AND t.production_year > 1986;

-- w032
SELECT MIN(t.title) AS t_title,
       MIN(kt.kind) AS kt_kind,
       MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     kind_type AS kt,
     kind_type AS lkt,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.kind_id = lkt.id
  AND akt.movie_id = mc.movie_id
  AND mc.note IS NULL
  AND t.title LIKE '%River%'
  AND lkt.kind IN ('pilot', 'feature', 'shortform');

-- w033
SELECT MIN(t.production_year) AS t_production_year,
       MIN(t.title) AS t_title
FROM complete_cast AS cc,
     movie_keyword AS mk,
     comp_cast_type AS subj,
     title AS t
WHERE cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND mk.movie_id = t.id
  AND cc.movie_id = mk.movie_id
  AND t.production_year BETWEEN 1968 AND 1981
  AND subj.kind = 'ensemble';

-- w034
SELECT MIN(chn.name) AS chn_name,
       MIN(cn.name) AS cn_name
FROM char_name AS chn,
     cast_info AS ci,
     company_name AS cn,
     company_type AS ct,
     kind_type AS kt,
     title AS lnk,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     role_type AS rt,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND ci.role_id = rt.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND ci.movie_id = mc.movie_id
  AND kt.kind IN ('miniseries', 'shortform')
  AND rt.role = 'stunt'
  AND mc.note IS NULL
  AND ci.note = '(archive)'
  AND lnk.production_year < 1995;

-- w035
SELECT MIN(kt.kind) AS kt_kind,
       MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year
FROM kind_type AS kt,
     movie_keyword AS mk,
     movie_link AS ml,
     title AS t
WHERE t.kind_id = kt.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND kt.kind IN ('serial', 'feature')
  AND t.production_year > 1998;

-- w036
SELECT MIN(chn.name) AS chn_name,
       MIN(t.title) AS t_title,
       MIN(mx.info) AS mx_info
FROM aka_title AS akt,
     complete_cast AS cc,
     char_name AS chn,
     cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     name AS n,
     role_type AS rt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.person_id = n.id
  AND ci.person_role_id = chn.id
  AND ci.role_id = rt.id
  AND cc.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND cc.movie_id = mx.movie_id
  AND mx.info > '7.5'
  AND rt.role = 'composer'
  AND ci.note = '(archive)'
  AND n.name LIKE '%Kai%';

-- w037
SELECT MIN(t.production_year) AS t_production_year,
       MIN(t.title) AS t_title,
       MIN(akt.title) AS akt_title
FROM aka_title AS akt,
     cast_info AS ci,
     movie_link AS ml,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ml.movie_id = t.id
  AND ci.movie_id = ml.movie_id
  AND akt.title LIKE '%Echo%'
  AND t.production_year > 2006;

-- w038
SELECT MIN(t.title) AS t_title,
       MIN(chn.name) AS chn_name
FROM complete_cast AS cc,
     char_name AS chn,
     cast_info AS ci,
     company_type AS ct,
     keyword AS k,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND mi.movie_id = mk.movie_id
  AND t.production_year BETWEEN 1969 AND 1974
  AND ct.kind = 'broadcaster'
  AND k.keyword = 'lighthouse'
  AND mxt.info IN ('score', 'ranking', 'tally')
  AND mi.info IN ('Thriller', 'Satire');

-- w039
SELECT MIN(t.title) AS t_title
FROM aka_title AS akt,
     cast_info AS ci,
     movie_keyword AS mk,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND mk.movie_id = t.id
  AND t.title LIKE '%Echo%'
  AND ci.note IN ('(cameo)', '(singing)')
  AND akt.title LIKE '%Harbor%';

-- w040
SELECT MIN(lt.link) AS lt_link,
       MIN(mx.info) AS mx_info
FROM aka_title AS akt,
     complete_cast AS cc,
     company_name AS cn,
     link_type AS lt,
     movie_companies AS mc,
     movie_info AS mi,
     info_type AS mit,
     movie_link AS ml,
     movie_info_idx AS mx,
     comp_cast_type AS stat,
     title AS t
WHERE akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mx.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND mx.movie_id = ml.movie_id
  AND mc.note LIKE '%(festival)%'
  AND stat.kind = 'partial';

-- w041
SELECT MIN(t.production_year) AS t_production_year
FROM complete_cast AS cc,
     cast_info AS ci,
     movie_link AS ml,
     title AS t
WHERE ci.movie_id = t.id
  AND cc.movie_id = t.id
  AND ml.movie_id = t.id
  AND ci.note IN ('(archive)', '(cameo)')
  AND t.production_year > 1985;

-- w042
SELECT MIN(lnk.title) AS lnk_title,
       MIN(t.production_year) AS t_production_year
FROM aka_title AS akt,
     complete_cast AS cc,
     char_name AS chn,
     cast_info AS ci,
     keyword AS k,
     title AS lnk,
     link_type AS lt,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND cc.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.production_year < 1982
  AND ci.note = '(cameo)'
  AND mx.info > '6.0';

-- w043
SELECT MIN(mc.note) AS mc_note,
       MIN(t.production_year) AS t_production_year,
       MIN(t.title) AS t_title
FROM movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     title AS t
WHERE mc.movie_id = t.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND t.production_year > 2012
  AND mc.note LIKE '%(festival)%';

-- w044
SELECT MIN(cn.name) AS cn_name,
       MIN(mc.note) AS mc_note
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     company_name AS cn,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     comp_cast_type AS stat,
     comp_cast_type AS subj,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND stat.kind = 'partial'
  AND t.production_year > 2000
  AND mx.info > '6.0';

-- w045
SELECT MIN(t.production_year) AS t_production_year
FROM cast_info AS ci,
     kind_type AS kt,
     movie_info AS mi,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND mi.movie_id = t.id
  AND mi.movie_id = ci.movie_id
  AND kt.kind IN ('serial', 'miniseries')
  AND mi.info IN ('Biopic', 'Mystery');

-- w046
SELECT MIN(lnk.title) AS lnk_title,
       MIN(mx.info) AS mx_info
FROM aka_title AS akt,
     cast_info AS ci,
     keyword AS k,
     title AS lnk,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND mc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND ci.movie_id = mk.movie_id
  AND akt.title LIKE '%Night%'
  AND mc.note LIKE '%(festival)%'
  AND mx.info > '4.5'
  AND lnk.production_year < 1970
  AND ci.note = '(cameo)';

-- w047
SELECT MIN(akt.title) AS akt_title
FROM aka_title AS akt,
     complete_cast AS cc,
     comp_cast_type AS subj,
     title AS t
WHERE akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND akt.movie_id = cc.movie_id
  AND subj.kind = 'crew'
  AND t.production_year BETWEEN 1963 AND 1974
  AND akt.title LIKE '%Night%';

-- w048
SELECT MIN(mc.note) AS mc_note,
       MIN(mx.info) AS mx_info,
       MIN(t.production_year) AS t_production_year
FROM aka_title AS akt,
     kind_type AS kt,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND mx.info > '8.0'
  AND mc.note LIKE '%(festival)%'
  AND lt.link IN ('homage', 'remake', 'spinoff')
  AND mi.info IN ('Thriller', 'Heist', 'Fable')
  AND akt.title LIKE '%Echo%';

-- w049
SELECT MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     movie_info AS mi,
     movie_keyword AS mk,
     title AS t
WHERE akt.movie_id = t.id
  AND mi.movie_id = t.id
  AND mk.movie_id = t.id
  AND akt.movie_id = mi.movie_id
  AND akt.title LIKE '%Night%'
  AND mi.info = 'Thriller'
  AND t.title LIKE '%Winter%';

-- w050
SELECT MIN(t.production_year) AS t_production_year
FROM aka_name AS an,
     cast_info AS ci,
     company_name AS cn,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     name AS n,
     role_type AS rt,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_id = n.id
  AND ci.role_id = rt.id
  AND an.person_id = n.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND mc.movie_id = mk.movie_id
  AND t.production_year > 2010
  AND an.name LIKE '%ra%';

-- w051
SELECT MIN(t.production_year) AS t_production_year,
       MIN(t.title) AS t_title,
       MIN(akt.title) AS akt_title
FROM aka_title AS akt,
     cast_info AS ci,
     movie_keyword AS mk,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND mk.movie_id = t.id
  AND akt.title LIKE '%Harbor%'
  AND t.title LIKE '%River%'
  AND ci.note = '(archive)';

-- w052
SELECT MIN(lnk.title) AS lnk_title,
       MIN(t.production_year) AS t_production_year,
       MIN(kt.kind) AS kt_kind
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     kind_type AS kt,
     kind_type AS lkt,
     title AS lnk,
     movie_companies AS mc,
     movie_link AS ml,
     role_type AS rt,
     comp_cast_type AS stat,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.kind_id = lkt.id
  AND stat.kind IN ('partial', 'verified')
  AND lkt.kind = 'serial'
  AND t.production_year BETWEEN 1980 AND 1985
  AND ci.note IN ('(archive)', '(cameo)')
  AND rt.role = 'lead';

-- w053
SELECT MIN(mx.info) AS mx_info
FROM kind_type AS kt,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     title AS t
WHERE t.kind_id = kt.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mx.movie_id = mk.movie_id
  AND mx.info > '6.0'
  AND kt.kind IN ('anthology', 'pilot', 'feature');

-- w054
SELECT MIN(t.production_year) AS t_production_year
FROM complete_cast AS cc,
     cast_info AS ci,
     company_name AS cn,
     movie_companies AS mc,
     movie_info AS mi,
     info_type AS mit,
     movie_info_idx AS mx,
     role_type AS rt,
     comp_cast_type AS stat,
     comp_cast_type AS subj,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mx.movie_id = t.id
  AND t.production_year BETWEEN 1966 AND 1973
  AND mi.info IN ('Heist', 'Western')
  AND subj.kind IN ('crew', 'ensemble')
  AND mx.info > '8.0'
  AND cn.country_code = '[kr]';

-- w055
SELECT MIN(mi.info) AS mi_info,
       MIN(akt.title) AS akt_title,
       MIN(t.production_year) AS t_production_year
FROM aka_title AS akt,
     movie_info AS mi,
     movie_link AS ml,
     title AS t
WHERE akt.movie_id = t.id
  AND mi.movie_id = t.id
  AND ml.movie_id = t.id
  AND t.title LIKE '%Winter%'
  AND akt.title LIKE '%Night%'
  AND mi.info IN ('Western', 'Fable');

-- w056
SELECT MIN(t.title) AS t_title,
       MIN(mc.note) AS mc_note,
       MIN(chn.name) AS chn_name
FROM aka_title AS akt,
     complete_cast AS cc,
     char_name AS chn,
     cast_info AS ci,
     company_name AS cn,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     name AS n,
     comp_cast_type AS subj,
     title AS t
WHERE akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.person_id = n.id
  AND ci.person_role_id = chn.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND subj.kind IN ('ensemble', 'crew')
  AND t.title LIKE '%Echo%'
  AND mc.note IS NULL
  AND cn.name LIKE '%Pine%'
  AND ci.note IN ('(archive)', '(cameo)');

-- w057
SELECT MIN(mi.info) AS mi_info,
       MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year
FROM char_name AS chn,
     cast_info AS ci,
     movie_info AS mi,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND mi.movie_id = t.id
  AND ci.note IN ('(singing)', '(archive)')
  AND mi.info = 'Heist'
  AND chn.name LIKE '%Widow%'
  AND t.production_year > 2001;

-- w058
SELECT MIN(mc.note) AS mc_note,
       MIN(akt.title) AS akt_title,
       MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     company_type AS ct,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     info_type AS mit,
     movie_keyword AS mk,
     role_type AS rt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mk.movie_id = t.id
  AND ct.kind IN ('laboratory', 'studio')
  AND akt.title LIKE '%Harbor%'
  AND kt.kind IN ('feature', 'shortform', 'anthology');

-- w059
SELECT MIN(mi.info) AS mi_info,
       MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year
FROM cast_info AS ci,
     movie_info AS mi,
     role_type AS rt,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND mi.movie_id = t.id
  AND t.title LIKE '%Winter%'
  AND rt.role = 'lead';

-- w060
SELECT MIN(n.name) AS n_name,
       MIN(mc.note) AS mc_note,
       MIN(an.name) AS an_name
FROM aka_title AS akt,
     aka_name AS an,
     cast_info AS ci,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     name AS n,
     role_type AS rt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.person_id = n.id
  AND ci.role_id = rt.id
  AND an.person_id = n.id
  AND mc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.movie_id = mi.movie_id
  AND mc.note IS NULL
  AND kt.kind IN ('shortform', 'feature')
  AND akt.title LIKE '%Night%'
  AND rt.role IN ('lead', 'extra', 'composer')
  AND n.name LIKE '%Vera%';

-- w061
SELECT MIN(mc.note) AS mc_note,
       MIN(t.production_year) AS t_production_year
FROM company_name AS cn,
     company_type AS ct,
     movie_companies AS mc,
     title AS t
WHERE mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND cn.country_code = '[za]'
  AND mc.note IS NULL
  AND ct.kind = 'studio';

-- w062
SELECT MIN(cn.name) AS cn_name
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     company_name AS cn,
     company_type AS ct,
     kind_type AS kt,
     movie_companies AS mc,
     movie_keyword AS mk,
     role_type AS rt,
     comp_cast_type AS stat,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND mk.movie_id = t.id
  AND mc.note IS NULL
  AND rt.role IN ('extra', 'narrator', 'stunt')
  AND akt.title LIKE '%Harbor%'
  AND t.production_year BETWEEN 1968 AND 1978
  AND stat.kind = 'partial';

-- w063
SELECT MIN(lnk.title) AS lnk_title
FROM aka_title AS akt,
     title AS lnk,
     movie_link AS ml,
     title AS t
WHERE akt.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND akt.title LIKE '%Harbor%'
  AND lnk.production_year < 1982
  AND t.production_year > 2002;

-- w064
SELECT MIN(pi.info) AS pi_info
FROM char_name AS chn,
     cast_info AS ci,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_link AS ml,
     movie_info_idx AS mx,
     name AS n,
     person_info AS pi,
     role_type AS rt,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_id = n.id
  AND ci.person_role_id = chn.id
  AND ci.role_id = rt.id
  AND pi.person_id = n.id
  AND mc.movie_id = t.id
  AND mx.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND t.title LIKE '%River%'
  AND lt.link = 'spinoff'
  AND mc.note IS NULL
  AND rt.role = 'narrator';

-- w065
SELECT MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year
FROM aka_title AS akt,
     kind_type AS kt,
     movie_companies AS mc,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND akt.movie_id = mc.movie_id
  AND t.title LIKE '%River%'
  AND mc.note IS NULL;

-- w066
SELECT MIN(t.title) AS t_title,
       MIN(mx.info) AS mx_info,
       MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     complete_cast AS cc,
     company_name AS cn,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     info_type AS mxt,
     comp_cast_type AS subj,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mk.movie_id = mc.movie_id
  AND t.title LIKE '%Stone%'
  AND mc.note IS NULL;

-- w067
SELECT MIN(chn.name) AS chn_name,
       MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year
FROM char_name AS chn,
     cast_info AS ci,
     movie_link AS ml,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND ml.movie_id = t.id
  AND ml.movie_id = ci.movie_id
  AND ci.note IN ('(archive)', '(singing)')
  AND chn.name LIKE '%Captain%';

-- w068
SELECT MIN(n.name) AS n_name
FROM char_name AS chn,
     cast_info AS ci,
     keyword AS k,
     title AS lnk,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     name AS n,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_id = n.id
  AND ci.person_role_id = chn.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND mk.movie_id = mx.movie_id
  AND lnk.production_year < 1992
  AND t.production_year > 1999
  AND ci.note IN ('(cameo)', '(archive)', '(singing)');

-- w069
SELECT MIN(t.production_year) AS t_production_year,
       MIN(mx.info) AS mx_info,
       MIN(kt.kind) AS kt_kind
FROM kind_type AS kt,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     title AS t
WHERE t.kind_id = kt.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mx.movie_id = mk.movie_id
  AND t.production_year > 1996
  AND kt.kind = 'serial';

-- w070
SELECT MIN(mi.info) AS mi_info,
       MIN(mx.info) AS mx_info,
       MIN(t.production_year) AS t_production_year
FROM cast_info AS ci,
     company_type AS ct,
     keyword AS k,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     title AS t
WHERE ci.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND mc.movie_id = ml.movie_id
  AND ci.note IN ('(singing)', '(archive)', '(cameo)')
  AND t.production_year BETWEEN 1989 AND 1997
  AND ct.kind IN ('financier', 'studio')
  AND mx.info > '8.0';

-- w071
SELECT MIN(t.title) AS t_title,
       MIN(mi.info) AS mi_info
FROM complete_cast AS cc,
     movie_info AS mi,
     comp_cast_type AS stat,
     title AS t
WHERE cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mi.movie_id = t.id
  AND cc.movie_id = mi.movie_id
  AND stat.kind IN ('partial', 'verified')
  AND t.production_year BETWEEN 1963 AND 1978
  AND mi.info = 'Musical';

-- w072
SELECT MIN(mi.info) AS mi_info,
       MIN(t.title) AS t_title,
       MIN(mx.info) AS mx_info
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     info_type AS mxt,
     comp_cast_type AS stat,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND cc.movie_id = mk.movie_id
  AND mx.info > '7.5'
  AND kt.kind = 'pilot'
  AND t.production_year BETWEEN 1977 AND 1988
  AND stat.kind IN ('verified', 'partial');

-- w073
SELECT MIN(mc.note) AS mc_note,
       MIN(t.production_year) AS t_production_year
FROM complete_cast AS cc,
     kind_type AS kt,
     movie_companies AS mc,
     title AS t
WHERE t.kind_id = kt.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND cc.movie_id = mc.movie_id
  AND mc.note LIKE '%(theatrical)%'
  AND kt.kind IN ('pilot', 'feature', 'anthology')
  AND t.production_year BETWEEN 1966 AND 1970;

-- w074
SELECT MIN(t.title) AS t_title,
       MIN(akt.title) AS akt_title
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     comp_cast_type AS stat,
     comp_cast_type AS subj,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ci.movie_id = mc.movie_id
  AND mi.info IN ('Western', 'Satire')
  AND ci.note IN ('(archive)', '(singing)', '(cameo)')
  AND mc.note LIKE '%(Norway)%'
  AND akt.title LIKE '%Harbor%';

-- w075
SELECT MIN(t.title) AS t_title,
       MIN(chn.name) AS chn_name,
       MIN(t.production_year) AS t_production_year
FROM char_name AS chn,
     cast_info AS ci,
     movie_link AS ml,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND ml.movie_id = t.id
  AND ci.movie_id = ml.movie_id
  AND t.production_year > 2008
  AND ci.note IN ('(singing)', '(cameo)')
  AND chn.name LIKE '%Captain%';

-- w076
SELECT MIN(mx.info) AS mx_info,
       MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     complete_cast AS cc,
     company_name AS cn,
     company_type AS ct,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mc.movie_id = akt.movie_id
  AND kt.kind = 'pilot'
  AND mx.info > '8.0'
  AND ct.kind IN ('studio', 'financier')
  AND t.title LIKE '%Night%'
  AND mxt.info IN ('ranking', 'tally', 'score');

-- w077
SELECT MIN(t.title) AS t_title,
       MIN(t.production_year) AS t_production_year,
       MIN(chn.name) AS chn_name
FROM complete_cast AS cc,
     char_name AS chn,
     cast_info AS ci,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND cc.movie_id = t.id
  AND ci.movie_id = cc.movie_id
  AND ci.note IN ('(archive)', '(singing)')
  AND t.production_year BETWEEN 1977 AND 1991
  AND chn.name LIKE '%Captain%';

-- w078
SELECT MIN(cn.name) AS cn_name
FROM aka_title AS akt,
     cast_info AS ci,
     company_name AS cn,
     company_type AS ct,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mx.movie_id = akt.movie_id
  AND mi.info IN ('Thriller', 'Fable', 'Biopic')
  AND akt.title LIKE '%Night%'
  AND mx.info > '6.0';

-- w079
SELECT MIN(t.title) AS t_title
FROM cast_info AS ci,
     movie_info AS mi,
     role_type AS rt,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND mi.movie_id = t.id
  AND mi.movie_id = ci.movie_id
  AND ci.note IN ('(cameo)', '(archive)')
  AND t.production_year > 1990
  AND mi.info IN ('Musical', 'Biopic', 'Fable')
  AND rt.role = 'stunt';

-- w080
SELECT MIN(t.production_year) AS t_production_year,
       MIN(akt.title) AS akt_title,
       MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     company_type AS ct,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     info_type AS mit,
     movie_keyword AS mk,
     comp_cast_type AS subj,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND mc.movie_id = t.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mk.movie_id = t.id
  AND cc.movie_id = ci.movie_id
  AND mit.info IN ('runtime', 'soundtrack')
  AND ci.note IN ('(cameo)', '(singing)')
  AND kt.kind = 'serial'
  AND akt.title LIKE '%Echo%'
  AND mi.info IN ('Western', 'Musical');

-- w081
SELECT MIN(kt.kind) AS kt_kind,
       MIN(t.title) AS t_title
FROM aka_title AS akt,
     complete_cast AS cc,
     kind_type AS kt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND kt.kind IN ('feature', 'pilot', 'miniseries')
  AND akt.title LIKE '%Night%'
  AND t.production_year > 1999;

-- w082
SELECT MIN(akt.title) AS akt_title,
       MIN(cn.name) AS cn_name,
       MIN(kt.kind) AS kt_kind
FROM aka_title AS akt,
     complete_cast AS cc,
     company_name AS cn,
     company_type AS ct,
     kind_type AS kt,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     comp_cast_type AS subj,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND cc.movie_id = akt.movie_id
  AND akt.title LIKE '%Harbor%'
  AND t.title LIKE '%Echo%';

-- w083
SELECT MIN(mx.info) AS mx_info,
       MIN(t.production_year) AS t_production_year,
       MIN(t.title) AS t_title
FROM cast_info AS ci,
     movie_link AS ml,
     movie_info_idx AS mx,
     title AS t
WHERE ci.movie_id = t.id
  AND mx.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.movie_id = mx.movie_id
  AND t.title LIKE '%Echo%'
  AND mx.info > '4.5';

-- w084
SELECT MIN(mx.info) AS mx_info,
       MIN(kt.kind) AS kt_kind
FROM complete_cast AS cc,
     company_name AS cn,
     kind_type AS kt,
     kind_type AS lkt,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_link AS ml,
     movie_info_idx AS mx,
     comp_cast_type AS stat,
     title AS t
WHERE t.kind_id = kt.id
  AND cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mx.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.kind_id = lkt.id
  AND cc.movie_id = mx.movie_id
  AND lt.link = 'spinoff'
  AND kt.kind IN ('shortform', 'pilot', 'feature')
  AND cn.country_code IN ('[ca]', '[kr]', '[no]')
  AND t.production_year > 1992;

-- w085
SELECT MIN(mi.info) AS mi_info,
       MIN(t.production_year) AS t_production_year
FROM kind_type AS kt,
     movie_info AS mi,
     movie_link AS ml,
     title AS t
WHERE t.kind_id = kt.id
  AND mi.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.movie_id = mi.movie_id
  AND t.production_year > 2001
  AND kt.kind IN ('shortform', 'pilot')
  AND mi.info = 'Mystery';

-- w086
SELECT MIN(kt.kind) AS kt_kind
FROM aka_title AS akt,
     complete_cast AS cc,
     cast_info AS ci,
     keyword AS k,
     kind_type AS kt,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     role_type AS rt,
     comp_cast_type AS subj,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND cc.movie_id = t.id
  AND cc.subject_id = subj.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND mi.movie_id = mk.movie_id
  AND mx.info > '4.5'
  AND akt.title LIKE '%Night%'
  AND t.title LIKE '%Echo%';

-- w087
SELECT MIN(kt.kind) AS kt_kind,
       MIN(chn.name) AS chn_name,
       MIN(t.production_year) AS t_production_year
FROM char_name AS chn,
     cast_info AS ci,
     kind_type AS kt,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND ci.person_role_id = chn.id
  AND ci.note IN ('(cameo)', '(archive)', '(singing)')
  AND kt.kind = 'feature'
  AND t.production_year > 2002
  AND chn.name LIKE '%Captain%';

-- w088
SELECT MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     complete_cast AS cc,
     kind_type AS kt,
     title AS lnk,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     comp_cast_type AS stat,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.linked_movie_id = lnk.id
  AND ml.movie_id = mi.movie_id
  AND mx.info > '8.0'
  AND mxt.info = 'score'
  AND lnk.production_year < 1996
  AND stat.kind IN ('verified', 'partial');

-- w089
SELECT MIN(t.production_year) AS t_production_year,
       MIN(kt.kind) AS kt_kind
FROM kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     title AS t
WHERE t.kind_id = kt.id
  AND mc.movie_id = t.id
  AND mi.movie_id = t.id
  AND t.production_year BETWEEN 1961 AND 1970
  AND kt.kind IN ('miniseries', 'special', 'pilot')
  AND mc.note LIKE '%(Norway)%'
  AND mi.info IN ('Mystery', 'Thriller');

-- w090
SELECT MIN(mi.info) AS mi_info,
       MIN(cn.name) AS cn_name,
       MIN(lnk.title) AS lnk_title
FROM aka_title AS akt,
     company_name AS cn,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND cn.name LIKE '%Harbor%'
  AND mx.info > '7.5'
  AND mi.info IN ('Musical', 'Fable')
  AND lt.link IN ('spinoff', 'remake', 'homage')
  AND akt.title LIKE '%Harbor%';

-- w091
SELECT MIN(t.title) AS t_title
FROM cast_info AS ci,
     movie_info AS mi,
     role_type AS rt,
     title AS t
WHERE ci.movie_id = t.id
  AND ci.role_id = rt.id
  AND mi.movie_id = t.id
  AND ci.movie_id = mi.movie_id
  AND rt.role = 'stunt'
  AND t.production_year BETWEEN 1999 AND 2007
  AND ci.note IN ('(cameo)', '(singing)')
  AND mi.info IN ('Biopic', 'Fable');

-- w092
SELECT MIN(kt.kind) AS kt_kind,
       MIN(lnk.title) AS lnk_title,
       MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     complete_cast AS cc,
     keyword AS k,
     kind_type AS kt,
     kind_type AS lkt,
     title AS lnk,
     link_type AS lt,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND mi.movie_id = t.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND lnk.kind_id = lkt.id
  AND akt.movie_id = cc.movie_id
  AND t.title LIKE '%Night%'
  AND lkt.kind IN ('miniseries', 'pilot')
  AND lt.link = 'remake'
  AND akt.title LIKE '%Echo%'
  AND mi.info IN ('Mystery', 'Heist');

-- w093
SELECT MIN(cn.name) AS cn_name,
       MIN(t.title) AS t_title,
       MIN(mc.note) AS mc_note
FROM company_name AS cn,
     movie_companies AS mc,
     movie_info AS mi,
     title AS t
WHERE mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mi.movie_id = t.id
  AND mc.movie_id = mi.movie_id
  AND t.production_year > 1997
  AND cn.name LIKE '%Pine%'
  AND mc.note LIKE '%(Norway)%'
  AND mi.info IN ('Musical', 'Fable');

-- w094
SELECT MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     complete_cast AS cc,
     company_type AS ct,
     movie_companies AS mc,
     movie_info AS mi,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     comp_cast_type AS stat,
     title AS t
WHERE akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND cc.status_id = stat.id
  AND mc.movie_id = t.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND mx.info > '7.5'
  AND mxt.info IN ('tally', 'score')
  AND ct.kind = 'laboratory';

-- w095
SELECT MIN(kt.kind) AS kt_kind
FROM kind_type AS kt,
     movie_companies AS mc,
     movie_link AS ml,
     title AS t
WHERE t.kind_id = kt.id
  AND mc.movie_id = t.id
  AND ml.movie_id = t.id
  AND ml.movie_id = mc.movie_id
  AND t.production_year BETWEEN 1964 AND 1969
  AND mc.note LIKE '%(Norway)%'
  AND kt.kind IN ('special', 'anthology', 'serial');

-- w096
SELECT MIN(akt.title) AS akt_title,
       MIN(mc.note) AS mc_note
FROM aka_title AS akt,
     complete_cast AS cc,
     company_name AS cn,
     company_type AS ct,
     kind_type AS kt,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE t.kind_id = kt.id
  AND akt.movie_id = t.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_id = cn.id
  AND mc.company_type_id = ct.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND ml.movie_id = t.id
  AND mk.movie_id = cc.movie_id
  AND mxt.info = 'score'
  AND ct.kind IN ('broadcaster', 'laboratory', 'financier')
  AND mc.note IS NULL
  AND akt.title LIKE '%Harbor%';

-- w097
SELECT MIN(akt.title) AS akt_title,
       MIN(mi.info) AS mi_info
FROM aka_title AS akt,
     movie_info AS mi,
     movie_keyword AS mk,
     title AS t
WHERE akt.movie_id = t.id
  AND mi.movie_id = t.id
  AND mk.movie_id = t.id
  AND t.title LIKE '%River%'
  AND akt.title LIKE '%Echo%'
  AND mi.info = 'Mystery';

-- w098
SELECT MIN(lnk.title) AS lnk_title
FROM complete_cast AS cc,
     company_type AS ct,
     keyword AS k,
     title AS lnk,
     link_type AS lt,
     movie_companies AS mc,
     movie_keyword AS mk,
     movie_link AS ml,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_type_id = ct.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND mk.keyword_id = k.id
  AND ml.movie_id = t.id
  AND ml.link_type_id = lt.id
  AND ml.linked_movie_id = lnk.id
  AND cc.movie_id = mk.movie_id
  AND lt.link = 'remake'
  AND mx.info > '6.0'
  AND t.production_year > 1995
  AND k.keyword IN ('caravan', 'glacier', 'orchard');

-- w099
SELECT MIN(akt.title) AS akt_title
FROM aka_title AS akt,
     company_type AS ct,
     movie_companies AS mc,
     title AS t
WHERE akt.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_type_id = ct.id
  AND mc.movie_id = akt.movie_id
  AND akt.title LIKE '%Echo%'
  AND mc.note IS NULL
  AND ct.kind = 'laboratory';

-- w100
SELECT MIN(mi.info) AS mi_info
FROM complete_cast AS cc,
     cast_info AS ci,
     company_type AS ct,
     kind_type AS kt,
     movie_companies AS mc,
     movie_info AS mi,
     info_type AS mit,
     movie_keyword AS mk,
     movie_info_idx AS mx,
     info_type AS mxt,
     title AS t
WHERE t.kind_id = kt.id
  AND ci.movie_id = t.id
  AND cc.movie_id = t.id
  AND mc.movie_id = t.id
  AND mc.company_type_id = ct.id
  AND mi.movie_id = t.id
  AND mi.info_type_id = mit.id
  AND mx.movie_id = t.id
  AND mx.info_type_id = mxt.id
  AND mk.movie_id = t.id
  AND ci.movie_id = mi.movie_id
  AND mi.info IN ('Satire', 'Fable')
  AND mc.note LIKE '%(Norway)%'
  AND mit.info = 'soundtrack'
  AND kt.kind IN ('feature', 'special')
  AND mxt.info = 'ranking';
