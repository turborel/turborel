# The Information Schema

[Contents](README.md)

The schema INFORMATION_SCHEMA describes the database in tables: its
schemas, tables, columns, constraints, privileges, roles and triggers,
what of the standard it supports, and, of turborel's
own, its indexes, settings and users. Every user may read it, and may
grant SELECT on views of it. It is read like any table, in the
session's transaction, as of its snapshot; it is never written.

Each view shows only what the session's user may see: an object it
owns, or holds a privilege on, or that PUBLIC does. Once the session
sets a role ([SET ROLE](statements/set-role.md)), each view shows too
what that role, and every role applicable for it, owns or holds a
privilege on, as the session's statements may use it.

```sql
SELECT TABLE_SCHEMA, TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'ANN';
```

Run by BOB, who holds no privilege on ANN's tables, it gives no row;
once ANN grants BOB SELECT on DEPT, it gives DEPT.

Its names are of the types SQL_IDENTIFIER, CHARACTER VARYING(128), and
its texts of CHARACTER_DATA, CHARACTER VARYING(32767); a text longer than
that, as a long view's definition, is shown null. The catalog is named
TURBOREL, unless the setting CATALOG.NAME names it otherwise.

## The views

The views of SQL-92, each with the columns SQL-92 gives it and those
ISO/IEC 9075-11 adds, described below:

| View | One row of each |
|---|---|
| INFORMATION_SCHEMA_CATALOG_NAME | the catalog: its name |
| SCHEMATA | schema the user owns |
| TABLES | table and view, its TABLE_TYPE BASE TABLE, VIEW, GLOBAL TEMPORARY, LOCAL TEMPORARY or SYSTEM VERSIONED *(extension)* |
| VIEWS | view: its definition, CHECK OPTION, whether updatable |
| COLUMNS | column of a table or view: its position, type, default, nullability, domain, identity and generation |
| DOMAINS | domain: its type and default |
| DOMAIN_CONSTRAINTS | constraint of a domain |
| TABLE_CONSTRAINTS | constraint of a table: its kind, PRIMARY KEY, UNIQUE, FOREIGN KEY or CHECK, whether deferrable, and of a UNIQUE whether NULLS DISTINCT |
| REFERENTIAL_CONSTRAINTS | foreign key: the constraint it references, its MATCH and actions |
| CHECK_CONSTRAINTS | CHECK, domain constraint and assertion: its condition |
| KEY_COLUMN_USAGE | column of a UNIQUE, PRIMARY KEY or foreign key, in order |
| ASSERTIONS | assertion: whether deferrable |
| TABLE_PRIVILEGES | privilege on a table |
| COLUMN_PRIVILEGES | privilege on a column |
| USAGE_PRIVILEGES | USAGE on a domain, a character set, a collation, a sequence generator, its OBJECT_TYPE SEQUENCE, a registered XML Schema, its OBJECT_TYPE XML SCHEMA, or a registered XQuery module, its OBJECT_TYPE XQUERY MODULE |
| VIEW_TABLE_USAGE | table a view reads |
| VIEW_COLUMN_USAGE | column a view reads |
| CONSTRAINT_TABLE_USAGE | table a constraint refers to |
| CONSTRAINT_COLUMN_USAGE | column a constraint refers to |
| COLUMN_DOMAIN_USAGE | column of a domain |
| CHARACTER_SETS | character set: SQL_TEXT, the one there is |
| COLLATIONS | the built-in collations, and those CREATE COLLATION made |
| TRANSLATIONS | translation: none |
| SQL_LANGUAGES | standard conformance is claimed to: ISO 9075 of 1987, SQL-86, at Level 2, by direct invocation |

Two views of ISO/IEC 9075-11 describe the parts of array and row types
*(extension)*, of the tables and domains the views above show:

| View | One row of each |
|---|---|
| ELEMENT_TYPES | array or multiset type of a column, a domain, a distinct type's source, a function's parameter or result, or a part of one, with its OBJECT_TYPE TABLE, DOMAIN, USER-DEFINED TYPE or ROUTINE, COLLECTION_TYPE_IDENTIFIER the collection type's DTD_IDENTIFIER, and its element type, as COLUMNS gives a type |
| FIELDS | field of a row type of a column, a domain or a part of either, with ROW_IDENTIFIER the row type's DTD_IDENTIFIER, FIELD_NAME, ORDINAL_POSITION, and the field's type |

Six more of ISO/IEC 9075-11 describe user-defined types and the
functions a distinct type's definition makes ([CREATE TYPE](statements/create-type.md)),
and the functions and procedures CREATE FUNCTION and CREATE PROCEDURE
make, those the user, or PUBLIC, holds a privilege on:

| View | One row of each |
|---|---|
| USER_DEFINED_TYPES | structured type: USER_DEFINED_TYPE_CATEGORY STRUCTURED, IS_INSTANTIABLE and IS_FINAL as defined, its ordering's form, category and function, NONE if it has none, and its reference type's representation in REFERENCE_TYPE. Distinct type: USER_DEFINED_TYPE_CATEGORY DISTINCT, IS_INSTANTIABLE and IS_FINAL YES, ORDERING_FORM FULL and ORDERING_CATEGORY MAP with the specific name of its function to its source in ORDERING_ROUTINE_CATALOG, _SCHEMA and _NAME, or NONE of an array or multiset source, and the source type, as COLUMNS gives a type, its DTD_IDENTIFIER in SOURCE_DTD_IDENTIFIER |
| UDT_PRIVILEGES | privilege on a user-defined type, its PRIVILEGE_TYPE TYPE USAGE or UNDER |
| ROUTINES | routine: its specific name and its name, ROUTINE_TYPE FUNCTION or PROCEDURE, a function's result type, as COLUMNS gives a type, the type's name in TYPE_UDT_CATALOG, _SCHEMA and _NAME, null of a procedure, ROUTINE_BODY SQL, ROUTINE_DEFINITION its body to the owner of its schema and null to others, IS_DETERMINISTIC, SQL_DATA_ACCESS CONTAINS SQL, READS SQL DATA or MODIFIES SQL DATA, a function's IS_NULL_CALL, MODULE_CATALOG, MODULE_SCHEMA and MODULE_NAME the module of a module's routine, null of another, SCHEMA_LEVEL_ROUTINE YES, or NO of a module's routine, and SECURITY_TYPE DEFINER. A distinct type's function is DETERMINISTIC, CONTAINS SQL and not null-call. A polymorphic table function is a FUNCTION of a null result type |
| PARAMETERS | routine's parameter: its ORDINAL_POSITION, from 1, PARAMETER_MODE IN, OUT or INOUT, its name and its type. [EXTENSION] A polymorphic table function's generic table parameter is of DATA_TYPE TABLE, its descriptor parameter DESCRIPTOR, their other type columns null |
| ROUTINE_PRIVILEGES | privilege on a routine, its PRIVILEGE_TYPE EXECUTE, the owner's of _SYSTEM among them |
| TRANSFORMS | transform: a distinct type's own, its GROUP_NAME the name of the function from its source, of TRANSFORM_TYPE FROM SQL, its function to its source, and TO SQL, its function from it, and each function of a group CREATE TRANSFORM made |

Eight more describe structured types and typed tables
([CREATE TYPE](statements/create-type.md), [CREATE TABLE](statements/create-table.md)):

| View | One row of each |
|---|---|
| ATTRIBUTES | attribute of a structured type: its name, ORDINAL_POSITION, default and type, as COLUMNS gives a type |
| DIRECT_SUPERTYPES | structured type and its direct supertype |
| METHOD_SPECIFICATIONS | method specification: its specific name, its name, IS_STATIC, IS_OVERRIDING, IS_CONSTRUCTOR, its result type and characteristics |
| METHOD_SPECIFICATION_PARAMETERS | parameter of a method specification, as PARAMETERS gives one |
| DIRECT_SUPERTABLES | typed table or referenceable view and its direct supertable or superview |
| REFERENCED_TYPES | reference type of a column, attribute, parameter or part of one: its referenced type, as ELEMENT_TYPES gives a part |
| TABLE_METHOD_PRIVILEGES | privilege of SELECT on a method of a typed table's type, granted or held by the user or an enabled role |
| ROLE_TABLE_METHOD_GRANTS | such a privilege granted to or by an enabled role |

Eight more of ISO/IEC 9075-11 describe roles ([CREATE
ROLE](statements/create-role.md)) and the privileges granted to them:

| View | One row of each |
|---|---|
| ENABLED_ROLES | role the session has enabled: the role SET ROLE set, each role granted to it or to PUBLIC, and each granted to one of those in turn. None until a role is set |
| APPLICABLE_ROLES | grant of a role to the user, to PUBLIC, or to an enabled role, and of a role to one of those roles in turn, set or not: GRANTEE, ROLE_NAME, IS_GRANTABLE YES if granted WITH ADMIN OPTION |
| ADMINISTRABLE_ROLE_AUTHORIZATIONS | grant of a role the user may grant, revoke and drop, holding it WITH ADMIN OPTION through APPLICABLE_ROLES: GRANTEE, ROLE_NAME, IS_GRANTABLE |
| ROLE_TABLE_GRANTS | privilege on a table granted to an enabled role, or by one, with the columns of TABLE_PRIVILEGES, WITH_HIERARCHY among them |
| ROLE_COLUMN_GRANTS | such a privilege on a column, with the columns of COLUMN_PRIVILEGES |
| ROLE_USAGE_GRANTS | such USAGE, with the columns of USAGE_PRIVILEGES |
| ROLE_UDT_GRANTS | such a privilege on a user-defined type, with the columns of UDT_PRIVILEGES |
| ROLE_ROUTINE_GRANTS | such a privilege on a routine, with the columns of ROUTINE_PRIVILEGES |

A role's creator holds it WITH ADMIN OPTION by a grant from `_SYSTEM`,
which APPLICABLE_ROLES shows. ANN, the owner of table STAFF, has granted
SELECT on it to the role CLERK, CLERK to the role SENIOR, and SENIOR to
BOB WITH ADMIN OPTION. Run by BOB:

```sql
SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'ANN';
SELECT ROLE_NAME FROM INFORMATION_SCHEMA.ENABLED_ROLES;
COMMIT;
SET ROLE 'SENIOR';
SELECT ROLE_NAME FROM INFORMATION_SCHEMA.ENABLED_ROLES ORDER BY 1;
SELECT GRANTEE, ROLE_NAME FROM INFORMATION_SCHEMA.APPLICABLE_ROLES ORDER BY 2;
SELECT GRANTEE, TABLE_NAME, PRIVILEGE_TYPE FROM INFORMATION_SCHEMA.ROLE_TABLE_GRANTS;
SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = 'ANN';
```

```
 TABLE_NAME
--------------------------------------------------------------
(0 rows)
 ROLE_NAME
--------------------------------------------------------------
(0 rows)
OK
OK
 ROLE_NAME
--------------------------------------------------------------
 CLERK
 SENIOR
(2 rows)
 GRANTEE                                                      | ROLE_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 SENIOR                                                       | CLERK
 BOB                                                          | SENIOR
(2 rows)
 GRANTEE                                                      | TABLE_NAME                                                   | PRIVILEGE_TYPE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 CLERK                                                        | STAFF                                                        | SELECT
(1 row)
 TABLE_NAME
--------------------------------------------------------------
 STAFF
(1 row)
```

Six more of ISO/IEC 9075-11 describe triggers ([CREATE
TRIGGER](statements/create-trigger.md)):

| View | One row of each |
|---|---|
| TRIGGERS | trigger on a table the user, or an enabled role, holds a privilege other than SELECT on: EVENT_MANIPULATION INSERT, DELETE or UPDATE, its subject table in EVENT_OBJECT_CATALOG, _SCHEMA and _TABLE, ACTION_ORDER its place, from 1, among the triggers of that table, event, time and orientation in the order they run, ACTION_CONDITION and ACTION_STATEMENT its WHEN condition and its action as written, to the owner of its schema and null to others, ACTION_ORIENTATION ROW or STATEMENT, ACTION_TIMING BEFORE, AFTER or INSTEAD OF, its transition names in ACTION_REFERENCE_OLD_TABLE, _NEW_TABLE, _OLD_ROW and _NEW_ROW, and CREATED, the CURRENT_TIMESTAMP of its CREATE TRIGGER, as ROUTINES's CREATED is below |
| TRIGGERED_UPDATE_COLUMNS | column of the column list of an UPDATE OF trigger, of a column the user holds a privilege other than SELECT on |
| TRIGGER_TABLE_USAGE | table or view a trigger's action reads, of a trigger of a schema the user, or an enabled role, owns |
| TRIGGER_COLUMN_USAGE | column a trigger's definition references in a search condition, its WHEN condition's or a WHERE's, or in a searched UPDATE's or DELETE's search condition or an UPDATE's source, or names in an INSERT's column list or an UPDATE's SET, its transition variables' among them, of such a trigger; not a column a query's select list alone references |
| TRIGGER_ROUTINE_USAGE | function or procedure a trigger's action invokes, by its specific name, of such a trigger |
| TRIGGER_SEQUENCE_USAGE | sequence generator a trigger's action draws from, of such a trigger |

Run by ANN, the owner of these:

```sql
CREATE TABLE ORDERS (ID INTEGER, AMOUNT INTEGER);
CREATE TABLE AUDIT (N INTEGER, ID INTEGER);
CREATE SEQUENCE S;
CREATE TRIGGER LOGGED AFTER INSERT ON ORDERS REFERENCING NEW ROW AS R FOR EACH ROW WHEN (R.AMOUNT > 100) INSERT INTO AUDIT VALUES (NEXT VALUE FOR S, R.ID);
CREATE TRIGGER CHANGED AFTER UPDATE OF AMOUNT ON ORDERS FOR EACH STATEMENT DELETE FROM AUDIT;
COMMIT;
SELECT TRIGGER_NAME, EVENT_MANIPULATION, ACTION_ORIENTATION, ACTION_CONDITION FROM INFORMATION_SCHEMA.TRIGGERS ORDER BY 1;
SELECT TRIGGER_NAME, EVENT_OBJECT_COLUMN FROM INFORMATION_SCHEMA.TRIGGERED_UPDATE_COLUMNS;
SELECT TRIGGER_NAME, SEQUENCE_NAME FROM INFORMATION_SCHEMA.TRIGGER_SEQUENCE_USAGE;
```

```
OK
OK
OK
OK
OK
OK
 TRIGGER_NAME                                                 | EVENT_MANIPULATION                                           | ACTION_ORIENTATION                                           | ACTION_CONDITION
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 CHANGED                                                      | UPDATE                                                       | STATEMENT                                                    |
 LOGGED                                                       | INSERT                                                       | ROW                                                          | R.AMOUNT > 100
(2 rows)
 TRIGGER_NAME                                                 | EVENT_OBJECT_COLUMN
--------------------------------------------------------------+--------------------------------------------------------------
 CHANGED                                                      | AMOUNT
(1 row)
 TRIGGER_NAME                                                 | SEQUENCE_NAME
--------------------------------------------------------------+--------------------------------------------------------------
 LOGGED                                                       | S
(1 row)
```

Five more of ISO/IEC 9075-4 describe SQL-server modules ([CREATE
MODULE](statements/create-module.md)):

| View | One row of each |
|---|---|
| MODULES | module the user, an enabled role or PUBLIC holds EXECUTE on: DEFAULT_CHARACTER_SET_CATALOG, _SCHEMA and _NAME SQL_TEXT's, DEFAULT_SCHEMA_CATALOG and _NAME its SCHEMA, MODULE_DEFINITION its text to the owner of its schema and null to others, MODULE_AUTHORIZATION null, SQL_PATH its PATH as written, null if none, and MODULE_CREATED and MODULE_LAST_ALTERED, the CURRENT_TIMESTAMP of its CREATE MODULE, the same, as no statement alters a module |
| MODULE_PRIVILEGES | privilege on a module, its PRIVILEGE_TYPE EXECUTE, the owner's of _SYSTEM among them, granted or granted by the user or an enabled role |
| ROLE_MODULE_GRANTS | such a privilege granted to or by an enabled role |
| MODULE_TABLE_USAGE | table or view a module's routine names, of a schema the user, or an enabled role, owns |
| MODULE_COLUMN_USAGE | column of such a table a module's routine names |

A module's declared temporary tables are in no view.

Run by ANN, the owner of these:

```sql
CREATE TABLE SALES (ID INTEGER, AMOUNT INTEGER);
CREATE MODULE SHOP
  PROCEDURE PLACE (IN ID INTEGER, IN AMOUNT INTEGER) MODIFIES SQL DATA INSERT INTO SALES VALUES (ID, AMOUNT);
  FUNCTION TOTAL () RETURNS INTEGER READS SQL DATA RETURN (SELECT SUM(AMOUNT) FROM SALES);
END MODULE;
GRANT EXECUTE ON MODULE SHOP TO PUBLIC;
COMMIT;
SELECT MODULE_NAME, DEFAULT_SCHEMA_NAME, SQL_PATH FROM INFORMATION_SCHEMA.MODULES;
SELECT GRANTOR, GRANTEE, PRIVILEGE_TYPE, IS_GRANTABLE FROM INFORMATION_SCHEMA.MODULE_PRIVILEGES ORDER BY 1;
SELECT MODULE_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.MODULE_COLUMN_USAGE ORDER BY 3;
```

```
OK
OK
OK
OK
 MODULE_NAME                                                  | DEFAULT_SCHEMA_NAME                                          | SQL_PATH
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 SHOP                                                         | ANN                                                          | NULL
(1 row)
 GRANTOR                                                      | GRANTEE                                                      | PRIVILEGE_TYPE                                               | IS_GRANTABLE
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 ANN                                                          | PUBLIC                                                       | EXECUTE                                                      | NO
 _SYSTEM                                                      | ANN                                                          | EXECUTE                                                      | YES
(2 rows)
 MODULE_NAME                                                  | TABLE_NAME                                                   | COLUMN_NAME
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 SHOP                                                         | SALES                                                        | AMOUNT
(1 row)
```

COLUMNS, ELEMENT_TYPES and FIELDS have three more columns, UDT_CATALOG,
UDT_SCHEMA and UDT_NAME, the name of a distinct type the column or part
is of, its DATA_TYPE then USER-DEFINED. They are null of another type.

COLUMNS has nine more columns *(extension)*, named as ISO/IEC 9075-11
names them. IS_IDENTITY is YES of an identity column. Of one,
IDENTITY_GENERATION is ALWAYS or BY DEFAULT, and IDENTITY_START,
IDENTITY_INCREMENT, IDENTITY_MAXIMUM and IDENTITY_MINIMUM are its
generator's options, each as text, and IDENTITY_CYCLE is YES if it cycles.
Of another column the six are null. IS_GENERATED is ALWAYS of a generated
column and NEVER of another, and GENERATION_EXPRESSION is a generated
column's expression as written. COLUMN_DEFAULT is null of both kinds.

COLUMNS has IS_HIDDEN last *(extension)*, YES of a pseudo-column of a
node or edge table, which `*` does not show, and NO of another
([CREATE TABLE](statements/create-table.md)).

COLUMNS and DOMAINS have two more *(extension)*, as ISO/IEC 9075-11
names them. DATA_TYPE is ARRAY, MULTISET or ROW of a column or domain
of such a type, MAXIMUM_CARDINALITY an array's maximum cardinality, and null of
another type. DTD_IDENTIFIER identifies the type's descriptor. It is a
column's ordinal position, or 1 of a domain, and null of a column of a domain,
whose type is the domain's. A part's is its parent's, a point, and the
field's place in its row, or 1 of an array's element, so the first field
of column 2 is `2.1` and the element of that field's array `2.1.1`.
ELEMENT_TYPES and FIELDS join to COLUMNS and DOMAINS by it.

The views that describe a type, COLUMNS, DOMAINS, ELEMENT_TYPES, FIELDS,
ATTRIBUTES, METHOD_SPECIFICATIONS, METHOD_SPECIFICATION_PARAMETERS,
ROUTINES and PARAMETERS, have seven more columns, as SQL/XML (ISO/IEC
9075-14) names them. Of an [XML type](19-xml.md), XML_PRIMARY_MODIFIER is
DOCUMENT, CONTENT or SEQUENCE, and XML_SECONDARY_MODIFIER is UNTYPED or
ANY, null of SEQUENCE, so a column declared XML alone shows SEQUENCE.
XML_SCHEMA_CATALOG, XML_SCHEMA_SCHEMA, XML_SCHEMA_NAME,
XML_SCHEMA_NAMESPACE and XML_SCHEMA_ELEMENT name the registered XML
Schema of a type of XMLSCHEMA, and its namespace and element if the type
names them, null of another XML type ([The XMLSCHEMA
types](19-xml.md#the-xmlschema-types)). All seven are null of a type that is
not XML. They follow the view's other columns, but in ROUTINES, where
they follow DTD_IDENTIFIER.

The views that describe a type have INTERVAL_TYPE and INTERVAL_PRECISION
after DATETIME_PRECISION, as ISO/IEC 9075-11 names them. Of an interval
type, DATA_TYPE is INTERVAL, INTERVAL_TYPE its fields, as `YEAR TO
MONTH` or `SECOND`, INTERVAL_PRECISION its leading field's precision, and
DATETIME_PRECISION its seconds' fractional digits; of another type both
are null.

TABLES has IS_INSERTABLE_INTO, YES of a base table and of an updatable
view, and NO of another view and of the Information Schema's own tables
and views, and COMMIT_ACTION, DELETE or PRESERVE of a temporary table as
its ON COMMIT says, and null of another. VIEWS has INSERTABLE_INTO, as
TABLES has it, and COLUMNS has IS_UPDATABLE, YES of each column of a
base table or an updatable view, NO of another. SCHEMATA has SQL_PATH,
null, as no schema is defined with a PATH. KEY_COLUMN_USAGE has
POSITION_IN_UNIQUE_CONSTRAINT: of a foreign key's column, the position,
in the key it references, of the column it references, its
ORDINAL_POSITION being its own position in the foreign key; null of a
UNIQUE or PRIMARY KEY constraint's. CHARACTER_SETS has CHARACTER_REPERTOIRE, SQL_TEXT, and
TRANSLATIONS has TRANSLATION_SOURCE_CATALOG, _SCHEMA and _NAME, of no
row.

ROUTINES has more columns ISO/IEC 9075-11 gives it. EXTERNAL_NAME,
EXTERNAL_LANGUAGE and PARAMETER_STYLE are an external routine's, so
null, as every routine is an SQL routine. SQL_PATH is the schemas a
routine's names are found in: its module's PATH, or its own schema,
each as `TURBOREL.ANN`. MAX_DYNAMIC_RESULT_SETS is 0. TO_SQL_SPECIFIC_
CATALOG, _SCHEMA and _NAME are null, as only an external routine has a
to-sql function. AS_LOCATOR is NO of a function and null of a
procedure. IS_UDT_DEPENDENT is YES of a routine a type's definition
makes, as an observer, and NO of another. The RESULT_CAST_ columns
describe a result cast's type, which only an external routine has, so
are null. PARAMETERS has FROM_SQL_SPECIFIC_ and TO_SQL_SPECIFIC_
CATALOG, _SCHEMA and _NAME, null for the same reason, and
METHOD_SPECIFICATIONS the RESULT_CAST_ columns, null.

ROUTINES and METHOD_SPECIFICATIONS have CREATED and LAST_ALTERED, of
the type TIMESTAMP(2) WITH TIME ZONE. CREATED is the value
CURRENT_TIMESTAMP had in the statement that made the routine or method
specification, in that session's time zone, to the hundredth of a
second. LAST_ALTERED is the same, as no statement alters a routine: ALTER
ROUTINE is of an external routine, and every routine here is an SQL
routine. The observers, mutators and constructor of a structured type,
and a distinct type's two functions, are of the CREATE TYPE, or of the
ALTER TYPE that added their attribute; a method's body is of its CREATE
METHOD, and its specification of the statement that specified it. They
are kept with the definitions, so a database opened again, recovered
after a crash, or followed by a standby shows the same. Run by ANN:

```sql
CREATE FUNCTION TWICE (X INTEGER) RETURNS INTEGER CONTAINS SQL RETURN X * 2;
COMMIT;
SELECT ROUTINE_NAME, CREATED <= CURRENT_TIMESTAMP AS MADE, LAST_ALTERED = CREATED AS UNALTERED FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_NAME = 'TWICE';
SELECT COLUMN_NAME, DATA_TYPE, DATETIME_PRECISION FROM INFORMATION_SCHEMA.COLUMNS WHERE TABLE_NAME = 'ROUTINES' AND COLUMN_NAME IN ('CREATED', 'LAST_ALTERED') ORDER BY ORDINAL_POSITION;
```

```
OK
OK
 ROUTINE_NAME                                                 | MADE  | UNALTERED
--------------------------------------------------------------+-------+-----------
 TWICE                                                        | TRUE  | TRUE
(1 row)
 COLUMN_NAME                                                  | DATA_TYPE                                                    | DATETIME_PRECISION
--------------------------------------------------------------+--------------------------------------------------------------+--------------------
 CREATED                                                      | TIMESTAMP WITH TIME ZONE                                     |                  2
 LAST_ALTERED                                                 | TIMESTAMP WITH TIME ZONE                                     |                  2
(2 rows)
```

ROUTINES and PARAMETERS end with twelve more, after the seven in
PARAMETERS. XML_CHAR_TYPE, XML_CHAR_MAX_LEN, XML_CHAR_OCT_LEN,
XML_CHAR_SET_CAT, XML_CHAR_SET_SCH, XML_CHAR_SET_NAME, XML_CHAR_COLL_CAT,
XML_CHAR_COLL_SCH, XML_CHAR_COLL_NAME and XML_CHAR_DTD_ID describe the
character string an external routine passes an XML value as, and
XML_OPTION says whether as DOCUMENT or CONTENT. Every routine is an SQL
routine, so they are null. XML_RETURN_MECHANISM in ROUTINES and
XML_PASSING_MECHANISM in PARAMETERS are BY REF of a result or parameter
of an XML type, which is passed by reference, and null of another.
ROUTINES and METHOD_SPECIFICATIONS have the seven XML columns of a result
cast's type too, named RESULT_CAST_XML_PRIMARY_MODIFIER and so on, before
those twelve in ROUTINES, and last in METHOD_SPECIFICATIONS; null, as
there is none. [XML Values](19-xml.md#the-type) has an example.

Three views of SQL/XML describe the registered XML Schemas the user, PUBLIC
or an enabled role holds USAGE on ([XML Values](19-xml.md#registered-xml-schemas)),
the permanently registered INFORMATION_SCHEMA.XS, XSI and SQLXML among
them, which every user does:

| View | One row of each |
|---|---|
| XML_SCHEMAS | registered XML Schema: XML_SCHEMA_TARGET_NAMESPACE, the zero-length string of none, XML_SCHEMA_LOCATION, XML_SCHEMA_CATALOG, _SCHEMA and _NAME, XML_SCHEMA_IS_DETERMINISTIC and XML_SCHEMA_IS_PERMANENT |
| XML_SCHEMA_NAMESPACES | namespace of one, its target namespace among them: the schema's target namespace, location and name, XML_SCHEMA_NAMESPACE and XML_SCHEMA_NAMESPACE_IS_DETERMINISTIC |
| XML_SCHEMA_ELEMENTS | global element declaration of one: the schema's target namespace, location and name, XML_SCHEMA_NAMESPACE, XML_SCHEMA_ELEMENT, its local name, and XML_SCHEMA_ELEMENT_IS_DETERMINISTIC |

An element declaration is not deterministic if it holds or refers to a
wildcard of any namespace, or of other than one, processed strictly or
laxly, as `<xs:any processContents="lax"/>`, at any depth; a namespace
is not if one of its element declarations is not, and a schema is not if
one of its namespaces is not. XML_SCHEMAS_S, XML_SCH_NSPACES_S and
XML_SCH_ELEMENTS_S show them too, under short names ([The short name
views](#the-short-name-views)).

TABLE_CONSTRAINTS has one more column *(extension)*, NULLS_DISTINCT, as
ISO/IEC 9075-11 names it: YES of a UNIQUE constraint of NULLS DISTINCT,
written or not, NO of one of NULLS NOT DISTINCT, and null of a PRIMARY
KEY and of every other kind.

NOT NULL is shown as a CHECK constraint, `C IS NOT NULL`. A constraint
given no name has one made for it: the table's name, its kind and a
number, as `EMPLOYEES_PRIMARY1`. A temporary table the session declares
is in no view.

The types, in COLUMNS and DOMAINS: a number's by the name it was
declared with, INTEGER, SMALLINT, BIGINT, DECIMAL or NUMERIC, of its precision
and scale, radix 10, or REAL, DOUBLE PRECISION or FLOAT, of the binary
precision it holds values of, 24 or 53, radix 2; a national character
string as CHARACTER, its octet length four octets a character.

## What depends on what

Ten views of ISO/IEC 9075-11 show what definitions depend on. Each
shows a row to the owner of one schema it names, or to an enabled role
that owns it, as the standard chooses: of a table or column a routine
names, the table's schema; of a routine a routine invokes, the invoked
routine's; of a sequence generator drawn from, the routine's; of what a
view invokes, the view's; of what a check constraint invokes, the
routine's; of a generated column, its table's; of a column of a
user-defined type, the type's; of a collation, its own.

| View | One row of each |
|---|---|
| ROUTINE_TABLE_USAGE | table or view a routine's body names: the routine's SPECIFIC_ and ROUTINE_ names, and the table's |
| ROUTINE_COLUMN_USAGE | column such a table's a routine's body references in a searched UPDATE's or DELETE's search condition or in an UPDATE's source, or names in an INSERT's column list or an UPDATE's SET, as the body binds now; not a column a query alone references |
| ROUTINE_ROUTINE_USAGE | routine a routine's body invokes: the routine's specific name in SPECIFIC_, the one invoked in ROUTINE_ |
| ROUTINE_SEQUENCE_USAGE | sequence generator a routine's body draws from |
| VIEW_ROUTINE_USAGE | routine a view's query invokes, by its specific name |
| CHECK_CONSTRAINT_ROUTINE_USAGE | routine an assertion's, a domain constraint's or a table check constraint's condition invokes |
| COLUMN_COLUMN_USAGE | column a generated column depends on: COLUMN_NAME the column, DEPENDENT_COLUMN the generated one |
| COLUMN_UDT_USAGE | column of a user-defined type, with the type |
| COLLATION_CHARACTER_SET_APPLICABILITY | collation the user's schemas hold, and SQL_TEXT, the character set it is of |
| DATA_TYPE_PRIVILEGES | data type descriptor the other views show the user: its object's name, OBJECT_TYPE TABLE, DOMAIN, ROUTINE or USER-DEFINED TYPE, and DTD_IDENTIFIER |

What a routine, view or constraint invokes is fixed as it is defined, as
the routine each invocation stands for is.

Run by ANN, the owner of these:

```sql
CREATE TABLE STOCK (ID INTEGER, QTY INTEGER, DOUBLED INTEGER GENERATED ALWAYS AS (QTY * 2));
CREATE FUNCTION TWICE (X INTEGER) RETURNS INTEGER DETERMINISTIC CONTAINS SQL RETURN X * 2;
CREATE FUNCTION TOTAL () RETURNS INTEGER READS SQL DATA RETURN (SELECT SUM(TWICE(QTY)) FROM STOCK);
CREATE VIEW BIG AS SELECT ID FROM STOCK WHERE TWICE(QTY) > 10;
CREATE PROCEDURE RESTOCK () MODIFIES SQL DATA UPDATE STOCK SET QTY = QTY + 1 WHERE ID > 0;
COMMIT;
SELECT ROUTINE_NAME, TABLE_NAME, COLUMN_NAME FROM INFORMATION_SCHEMA.ROUTINE_COLUMN_USAGE ORDER BY 3;
SELECT TABLE_NAME FROM INFORMATION_SCHEMA.VIEW_ROUTINE_USAGE;
SELECT COLUMN_NAME, DEPENDENT_COLUMN FROM INFORMATION_SCHEMA.COLUMN_COLUMN_USAGE;
```

```
OK
OK
OK
OK
OK
OK
 ROUTINE_NAME                                                 | TABLE_NAME                                                   | COLUMN_NAME
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 RESTOCK                                                      | STOCK                                                        | ID
 RESTOCK                                                      | STOCK                                                        | QTY
(2 rows)
 TABLE_NAME
--------------------------------------------------------------
 BIG
(1 row)
 COLUMN_NAME                                                  | DEPENDENT_COLUMN
--------------------------------------------------------------+--------------------------------------------------------------
 QTY                                                          | DOUBLED
(1 row)
```

## What turborel supports

Six views of ISO/IEC 9075-11 say what of the standard turborel supports,
and the limits it keeps:

| View | One row of each |
|---|---|
| SQL_FEATURES | feature and subfeature of ISO/IEC 9075-2 (Foundation), 9075-4 (SQL/PSM), 9075-11 and 9075-14 (SQL/XML): FEATURE_ID and FEATURE_NAME, a subfeature's SUB_FEATURE_ID and SUB_FEATURE_NAME, a single space of a feature; IS_SUPPORTED YES of what [CONFORMANCE.md](../../CONFORMANCE.md#features-claimed) claims, and of each subfeature of a feature it claims, else NO; IS_VERIFIED_BY null; COMMENTS the section of CONFORMANCE.md that says how it is done |
| SQL_PARTS | part of ISO/IEC 9075: PART its number, NAME, IS_SUPPORTED NO, as none is claimed whole |
| SQL_PACKAGES | package of features: none, as the standard's parts define none |
| SQL_SIZING | limit ISO/IEC 9075-2 leaves to the implementation: SIZING_ID its number there, IL001 to IL208, SIZING_NAME its words, SUPPORTED_VALUE the limit, 0 if there is none, null of what turborel has no use for, and COMMENTS what the value is of |
| SQL_IMPLEMENTATION_INFO | item of information on the implementation: none, as ISO/IEC 9075-2 and 9075-11 name none; those of the Call-Level Interface, ISO/IEC 9075-3, are not defined |
| SQL_SIZING_PROFILES | limit a profile of the standard requires: none, as no profile is defined |

Each feature is named as its part names it. The rows of SQL_FEATURES
are made from CONFORMANCE.md's table of the features claimed, so the two
always agree. Run by any user:

```sql
SELECT FEATURE_ID, SUB_FEATURE_ID, IS_SUPPORTED, COMMENTS FROM INFORMATION_SCHEMA.SQL_FEATURES
    WHERE FEATURE_ID IN ('T011', 'T031', 'F501') ORDER BY FEATURE_ID, SUB_FEATURE_ID;
SELECT PART, NAME, IS_SUPPORTED FROM INFORMATION_SCHEMA.SQL_PARTS
    WHERE PART IN ('2', '11', '14') ORDER BY CAST(PART AS INTEGER);
SELECT SIZING_ID, SUPPORTED_VALUE, COMMENTS FROM INFORMATION_SCHEMA.SQL_SIZING
    WHERE SIZING_ID IN (1, 2, 9, 39, 66, 208) ORDER BY SIZING_ID;
```

```
 FEATURE_ID                                                   | SUB_FEATURE_ID                                               | IS_SUPPORTED                                                 | COMMENTS
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 F501                                                         |                                                              | YES                                                          | CONFORMANCE.md, Transactions, sessions and the Information Schema
 F501                                                         | 01                                                           | YES                                                          | CONFORMANCE.md, Transactions, sessions and the Information Schema
 F501                                                         | 02                                                           | YES                                                          | CONFORMANCE.md, Transactions, sessions and the Information Schema
 T011                                                         |                                                              | YES                                                          | CONFORMANCE.md, Transactions, sessions and the Information Schema
 T031                                                         |                                                              | YES                                                          | CONFORMANCE.md, Truth values
(5 rows)
 PART                                                         | NAME                                                         | IS_SUPPORTED
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 2                                                            | Foundation (SQL/Foundation)                                  | NO
 11                                                           | Information and Definition Schemas (SQL/Schemata)            | NO
 14                                                           | XML-Related Specifications (SQL/XML)                         | NO
(3 rows)
    SIZING_ID |      SUPPORTED_VALUE | COMMENTS
--------------+----------------------+--------------------------------------------------------------
            1 |                32767 | IL001, in characters
            2 |           4294967296 | IL002, in characters
            9 |                   18 | IL009, in decimal digits
           39 |                    0 | IL039: none of an item parsed; an object or array constructed holds IL051's and IL052's
           66 |                 NULL | IL066: no externally-invoked procedure
          208 |                 1024 | IL208
(6 rows)
```

## The short name views

ISO/IEC 9075-11 defines most views again under a short name, and their
columns too, and ISO/IEC 9075-4 and SQL/XML (ISO/IEC 9075-14) define
those of theirs, for SQL written for a system whose names are 18
characters at most, as one without Feature F391 (Long identifiers) has.
Each short
name view shows the rows its view shows the same user, column for
column, in the order the standard gives them. A column whose name is
longer is renamed: CHARACTER_MAXIMUM_LENGTH is CHAR_MAX_LENGTH,
NUMERIC_PRECISION_RADIX NUMERIC_PREC_RADIX, MAXIMUM_CARDINALITY
MAX_CARDINALITY, XML_SCHEMA_NAMESPACE XML_SCHEMA_NSPACE, and so on.

| Short name view | Shows what this view shows |
|---|---|
| CATALOG_NAME | INFORMATION_SCHEMA_CATALOG_NAME |
| SCHEMATA_S | SCHEMATA |
| DOMAINS_S | DOMAINS |
| TABLES_S | TABLES |
| COLUMNS_S | COLUMNS |
| REF_CONSTRAINTS | REFERENTIAL_CONSTRAINTS |
| KEY_COLUMN_USAGE_S | KEY_COLUMN_USAGE |
| CHARACTER_SETS_S | CHARACTER_SETS |
| COLLATIONS_S | COLLATIONS |
| SEQUENCES_S | SEQUENCES |
| TRANSLATIONS_S | TRANSLATIONS |
| CONSTR_TABLE_USAGE | CONSTRAINT_TABLE_USAGE |
| CONSTR_COL_USAGE | CONSTRAINT_COLUMN_USAGE |
| COL_DOMAIN_USAGE | COLUMN_DOMAIN_USAGE |
| ELEMENT_TYPES_S | ELEMENT_TYPES |
| FIELDS_S | FIELDS |
| UDT_S | USER_DEFINED_TYPES |
| ROUTINES_S | ROUTINES |
| PARAMETERS_S | PARAMETERS |
| ATTRIBUTES_S | ATTRIBUTES |
| METHOD_SPECS | METHOD_SPECIFICATIONS |
| METHOD_SPEC_PARAMS | METHOD_SPECIFICATION_PARAMETERS |
| REFERENCED_TYPES_S | REFERENCED_TYPES |
| TABLE_METHOD_PRIVS | TABLE_METHOD_PRIVILEGES |
| ADMIN_ROLE_AUTHS | ADMINISTRABLE_ROLE_AUTHORIZATIONS |
| ROLE_ROUT_GRANTS | ROLE_ROUTINE_GRANTS |
| ROL_TAB_METH_GRNTS | ROLE_TABLE_METHOD_GRANTS |
| TRIGGERS_S | TRIGGERS |
| TRIG_UPDATE_COLS | TRIGGERED_UPDATE_COLUMNS |
| TRIG_TABLE_USAGE | TRIGGER_TABLE_USAGE |
| TRIG_COLUMN_USAGE | TRIGGER_COLUMN_USAGE |
| TRIG_ROUT_USAGE_S | TRIGGER_ROUTINE_USAGE |
| TRIG_SEQ_USAGE_S | TRIGGER_SEQUENCE_USAGE |
| MODULE_COL_USAGE | MODULE_COLUMN_USAGE |
| MODULES_S | MODULES |
| COLLATION_APPLIC_S | COLLATION_CHARACTER_SET_APPLICABILITY |
| COL_COL_USAGE | COLUMN_COLUMN_USAGE |
| CONSTR_ROUT_USE_S | CHECK_CONSTRAINT_ROUTINE_USAGE |
| ROUT_ROUT_USAGE_S | ROUTINE_ROUTINE_USAGE |
| ROUT_SEQ_USAGE_S | ROUTINE_SEQUENCE_USAGE |
| ROUTINE_COL_USAGE | ROUTINE_COLUMN_USAGE |
| ROUT_TABLE_USAGE | ROUTINE_TABLE_USAGE |
| XML_SCHEMAS_S | XML_SCHEMAS |
| XML_SCH_NSPACES_S | XML_SCHEMA_NAMESPACES |
| XML_SCH_ELEMENTS_S | XML_SCHEMA_ELEMENTS |
| SQL_IMPL_INFO | SQL_IMPLEMENTATION_INFO |
| SQL_SIZING_PROFS | SQL_SIZING_PROFILES |

A short name view has every column the standard gives it.

The views that describe a type end with the seven columns of an XML
type, named XML_PRIMARY_MOD, XML_SECONDARY_MOD, XML_SCHEMA_CATALOG,
XML_SCHEMA_SCHEMA, XML_SCHEMA_NAME, XML_SCHEMA_NSPACE and
XML_SCHEMA_ELEMENT; ROUTINES_S has them after DTD_IDENTIFIER, named
XML_PRIMARY_MOD, XML_SECOND_MOD, XML_SCHEMA_CAT, XML_SCH_SCH,
XML_SCH_NAME, XML_SCH_NSPACE and XML_SCH_ELEMENT, as SQL/XML names them.
ROUTINES_S and METHOD_SPECS have those of a result cast's type, named
RC_XML_PRIMARY_MOD, RC_XML_SECOND_MOD, RC_XML_SCHEMA_CAT, RC_XML_SCH_SCH,
RC_XML_SCH_NAME, RC_XML_SCH_NSPACE and RC_XML_SCH_ELEMENT.
ROUTINES_S and PARAMETERS_S end with the columns of the associated
string type, then XML_RETURN_MECH and XML_PASSING_MECH. MODULES_S calls
DEFAULT_SCHEMA_NAME DEFAULT_SCHEMA, and the timestamps CREATED and
ALTERED.

Run by ANN, the owner of these:

```sql
CREATE TABLE PARTS (ID INTEGER PRIMARY KEY, NAME CHARACTER VARYING(40), RADIUS DECIMAL(6,2));
COMMIT;
SELECT COLUMN_NAME, DATA_TYPE, CHAR_MAX_LENGTH, NUMERIC_PRECISION, NUMERIC_PREC_RADIX FROM INFORMATION_SCHEMA.COLUMNS_S
    WHERE TABLE_NAME = 'PARTS' ORDER BY ORDINAL_POSITION;
SELECT TABLE_NAME, TABLE_TYPE, SELF_REF_COL_NAME FROM INFORMATION_SCHEMA.TABLES_S WHERE TABLE_NAME = 'PARTS';
SELECT CATALOG_NAME FROM INFORMATION_SCHEMA.CATALOG_NAME;
```

```
OK
OK
 COLUMN_NAME                                                  | DATA_TYPE                                                    |      CHAR_MAX_LENGTH | NUMERIC_PRECISION | NUMERIC_PREC_RADIX
--------------------------------------------------------------+--------------------------------------------------------------+----------------------+-------------------+--------------------
 ID                                                           | INTEGER                                                      |                 NULL |                10 |                 10
 NAME                                                         | CHARACTER VARYING                                            |                   40 |              NULL |               NULL
 RADIUS                                                       | DECIMAL                                                      |                 NULL |                 6 |                 10
(3 rows)
 TABLE_NAME                                                   | TABLE_TYPE                                                   | SELF_REF_COL_NAME
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------
 PARTS                                                        | BASE TABLE                                                   | NULL
(1 row)
 CATALOG_NAME
--------------------------------------------------------------
 TURBOREL
(1 row)
```

## This implementation's own views *(extension)*

| View | Columns | One row of each |
|---|---|---|
| TABLE_OPTIONS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ENGINE_NAME, OPTION_NAME, DECLARED_BY, OPTION_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, CHANGED_BY, DESCRIPTION | option of a table's storage engine, its value set or its default |
| PERIODS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, PERIOD_NAME, START_COLUMN_NAME, END_COLUMN_NAME, CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME | period of a table the user holds a privilege on, SYSTEM_TIME or an application-time period: its start and end columns, and the period constraint of an application-time period ([CREATE TABLE](statements/create-table.md)) |
| INDEXES | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, METHOD_NAME, IS_UNIQUE, COLUMN_NAMES | index, a UNIQUE or PRIMARY KEY constraint's among them |
| SEQUENCES | SEQUENCE_CATALOG, SEQUENCE_SCHEMA, SEQUENCE_NAME, DATA_TYPE, NUMERIC_PRECISION, NUMERIC_PRECISION_RADIX, NUMERIC_SCALE, START_VALUE, MINIMUM_VALUE, MAXIMUM_VALUE, INCREMENT, CYCLE_OPTION | sequence generator the user holds USAGE on: its type, and its start, bounds and increment, each as text, and YES if it cycles ([CREATE SEQUENCE](statements/create-sequence.md)) |
| INDEX_OPTIONS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, METHOD_NAME, OPTION_NAME, DECLARED_BY, OPTION_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, CHANGED_BY, DESCRIPTION | option of an index |
| ADJACENCY_INDEXES | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, INDEX_NAME, COLUMN_NAMES, SOURCE_COLUMNS, INCLUDED_COLUMNS, DIRECTION, CSR_BYTES | `adjacency` index: how many of its columns are the source, how many are included, which ways it keeps the edges, and the most bytes its compacted adjacency holds ([CREATE INDEX](statements/create-index.md)) |
| SETTINGS | SETTING_NAME, SETTING_VALUE, VALUE_SOURCE, DEFAULT_VALUE, VALUE_KIND, IS_SESSION_SETTABLE, TAKES_EFFECT, DESCRIPTION, STARTED_VALUE, IS_PENDING | setting, its value for the session; of one taken at the next start, the value it started with, or null, and whether the value set waits for a restart |
| USERS | USER_NAME, IS_OWNER, CREATE_ROLE | user: to the owner every one, to another user itself; CREATE_ROLE YES if it may create a role |
| MEMORY_RESERVATIONS | RESERVED_BY, BYTES | part of the memory budget reserved, as a buffer pool's |
| CONNECTIONS | ID, USER_NAME, PEER, SECURE, STATE | connection a server serves: its user once authenticated, or NULL; where it came from, an address or `local`; whether it is secure; LOGIN, IDLE or RUNNING a statement |
| SERVER | WORKERS_BUSY, STATEMENTS_WAITING, CONNECTIONS, REFUSED, CLOSED_UNTOLD | one row: the workers running a statement, the statements waiting for one, the connections, and those refused past SERVER.MAX_CONNECTIONS, told why or closed at once |
| REPLICAS | MEMBER, ADDRESS, IS_SELF, ROLE, EPOCH, VIEW_NUMBER, IS_CONNECTED, LAST_HEARD_MS | member of the database's replica group ([Replication](../operator/09-replication.md)), in the epoch this node is in, to the owner: its number and address; whether it is this node; PRIMARY, BACKUP or UNKNOWN in the view this node knows; the epoch, and that view, or NULL; whether this node's link to it is up; the milliseconds since a frame from it arrived, or NULL |
| FILES | PATH, KIND, CLASS, IS_OPEN, READS, WRITES, SYNCS | file of the database: DATA, SHADOW (a side file of the data file, or of a plug-in's shadowed file), CATALOG, LOG, UNDO, SPILL, TRANSFER (a state transfer being fetched by a replica), PLUGIN (a plug-in's own), SERVER (its server's own: the diagnostics log, the socket, the TLS and key files) or OTHER; its class of durability, DURABLE (DATA, CATALOG, LOG, PLUGIN), DURABLE AT COMMIT (SHADOW), UNSYNCED (UNDO), SCRATCH (SPILL, TRANSFER), or null for a file not the database's; whether it is open now; its reads, writes and syncs |
| FILE_FORMATS | KIND, IS_SETTLED, BEHIND | kind of file of a database kept in a directory, DATA, LOG or CATALOG, to its owner: whether every one is in the formats STORAGE.ENCODINGS and STORAGE.CHECKSUM_KEY name; and what is still in others: the pages of the data file the job `turborel-rekey` has yet to look at, the log's segments, the catalog's file ([The files' formats](../operator/07-encryption.md#the-files-formats)) |
| GRAPH_ALGO_RUNS | RUN_ID, ALGORITHM, ARGUMENTS, USER_NAME, STARTED, ELAPSED_MICROS, VERTICES, EDGES, ROWS_RETURNED, THREADS, STATUS, MESSAGE | run of a function of GRAPH_ALGO *(extension)*, of the latest 1000, with its number from 1, its algorithm, its value arguments as `NAME => value`, who ran it, when it started and the microseconds it took, its graph's vertices and edges, its result's rows, the threads it ran on, DONE or FAILED, and a failed run's message or NULL; each user's own, every run of the database's owner ([Graph Algorithms](21-graph-algorithms.md)) |
| JOBS | LABEL, STATE, THREADED, RUNS, RESTARTS, LAST_ERROR | job of the database's background work, `turborel-checkpoint`, `turborel-maintenance`, `turborel-log-sync`, `turborel-read-ahead`, `turborel-assertion-totals`, `turborel-rekey`, `turborel-replica`, `turborel-archive` or a plug-in's `turborel-<kind>`: IDLE, DUE or RUNNING; whether a thread of its own runs it; how often it ran, and failed by a fault; its last error, or NULL |
| ASSERTION_TOTALS | CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME, FORM_NUMBER, KIND, VALUE_COUNT, NULL_COUNT, TOTAL, ROWS_KEPT | total an assertion of a schema of the user's keeps, as committed: its form's number, SUM, AVG, COUNT or NULLS (of a set function's argument that may be null); the values counted, or NULL of NULLS; their sum, as text, or NULL of COUNT and NULLS; the nulls; and the rows keeping them, which the job `turborel-assertion-totals` folds into one |
| PROPERTY_GRAPHS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, PROPERTY_GRAPH_MODE, IS_INVOKER_RIGHTS, IS_VALID, PROPERTY_GRAPH_DEFINITION | property graph the user, an enabled role or PUBLIC holds SELECT on: ENFORCED or TRUSTED, YES if WITH INVOKER RIGHTS, NO if INVALID until compiled, and its definition as a CREATE PROPERTY GRAPH statement to the owner of its schema, null to others ([CREATE PROPERTY GRAPH](statements/create-property-graph.md)) |
| PG_ELEMENT_TABLES | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, ELEMENT_TABLE_KIND, TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION, IS_UNDIRECTED, ELEMENT_TABLE_QUERY | element table of such a graph: VERTEX or EDGE, its table, its place among the graph's of its kind, YES of an UNDIRECTED edge table, and of an element table of a query's rows its query, the table's columns null |
| PG_LABELS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, LABEL_NAME, ORDINAL_POSITION | label of such an element table, in order |
| PG_PROPERTIES | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, LABEL_NAME, PROPERTY_NAME, ORDINAL_POSITION, PROPERTY_EXPRESSION, DATA_TYPE | property of such a label, in order: its expression as kept, and its declared type |
| PG_KEY_COLUMNS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, ELEMENT_TABLE_ALIAS, COLUMN_NAME, ORDINAL_POSITION | column of such an element table's key, in order |
| PG_EDGE_ENDPOINTS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, EDGE_TABLE_ALIAS, EDGE_END, VERTEX_TABLE_ALIAS, ORDINAL_POSITION, EDGE_COLUMN_NAME, VERTEX_COLUMN_NAME | column of such an edge table that joins it, at its SOURCE or DESTINATION end, to a vertex table, with the vertex table's column it matches |
| PROPERTY_GRAPH_PRIVILEGES | GRANTOR, GRANTEE, PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, PRIVILEGE_TYPE, IS_GRANTABLE | SELECT on a graph, the owner's of _SYSTEM among them, granted to or by the user, PUBLIC or an enabled role |
| GRAPH_STATISTICS | PROPERTY_GRAPH_CATALOG, PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME, STATISTIC_KIND, ELEMENT_TABLE_ALIAS, PROPERTY_NAME, SOURCE_LABEL, EDGE_LABEL, DESTINATION_LABEL, ROW_COUNT, DISTINCT_VALUES, NULL_FRACTION, OUT_DEGREE_AVERAGE, OUT_DEGREE_MEDIAN, OUT_DEGREE_P90, OUT_DEGREE_P99, OUT_DEGREE_MAXIMUM, IN_DEGREE_AVERAGE, IN_DEGREE_MEDIAN, IN_DEGREE_P90, IN_DEGREE_P99, IN_DEGREE_MAXIMUM | statistic of such a graph the planner reads *(extension)*: VERTEX or EDGE, of an element table, its rows and an edge table's degrees; PROPERTY, of a property; TRANSITION, of a source, edge and destination label, its edges; NULL while not known ([Graph statistics](#graph-statistics)) |
| GRAPH_TABLES | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, GRAPH_TABLE_TYPE | node or edge table the user holds a privilege on: NODE or EDGE ([CREATE TABLE](statements/create-table.md)) |
| CONNECTION_CONSTRAINTS | CONSTRAINT_CATALOG, CONSTRAINT_SCHEMA, CONSTRAINT_NAME, TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, ORDINAL_POSITION, FROM_TABLE_CATALOG, FROM_TABLE_SCHEMA, FROM_TABLE_NAME, TO_TABLE_CATALOG, TO_TABLE_SCHEMA, TO_TABLE_NAME, DELETE_RULE | clause of a CONNECTION constraint of such an edge table, in order: the node tables its edges may join, and CASCADE or NO ACTION |
| VECTOR_COLUMNS | TABLE_CATALOG, TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, ORDINAL_POSITION, VECTOR_DIMENSIONS, VECTOR_FORMAT, VECTOR_LAYOUT | VECTOR column of a table the user holds a privilege on: the dimension count, format and layout its type fixes, NULL of one it leaves open *(extension)* ([Vectors](14-vectors.md)) |
| VECTOR_DISTANCE_FUNCTIONS | FUNCTION_CATALOG, FUNCTION_SCHEMA, FUNCTION_NAME, SPECIFIC_NAME, LANGUAGE, IS_DETERMINISTIC, SYMMETRIC, NON_NEGATIVE, TRIANGLE_INEQUALITY, FORMATS, FUNCTION_OWNER, DEFINITION | distance function the user holds EXECUTE on: its language, SQL; YES of each property it declares; the formats it measures, separated by commas; its owner; and its body, or NULL if longer than 4,096 characters *(extension)* ([CREATE VECTOR DISTANCE FUNCTION](statements/create-vector-distance-function.md)) |
| EMBEDDING_MODELS | MODEL_CATALOG, MODEL_SCHEMA, MODEL_NAME, MODEL_TYPE, DIMENSIONS, ELEMENT_FORMAT, LAYOUT, MODEL_VERSION, MODEL_FILE, IS_REGISTERED, MODEL_OWNER | embedding model the user owns or holds EXECUTE on: its plug-in; the dimension count, format and layout of its vectors; its version and file, or NULL; YES if its plug-in is registered; and its owner. Its options are not shown, as one may hold a secret *(extension)* ([CREATE EMBEDDING MODEL](statements/create-embedding-model.md)) |
| EMBEDDING_MODEL_PRIVILEGES | GRANTOR, GRANTEE, MODEL_CATALOG, MODEL_SCHEMA, MODEL_NAME, PRIVILEGE_TYPE, IS_GRANTABLE | grant of EXECUTE on an embedding model that the user made or holds, or PUBLIC holds, or of a model of a schema the user owns, as ROUTINE_PRIVILEGES shows a routine's; the owner's own granted by _SYSTEM *(extension)* |
| VECTOR_PIPELINES | PIPELINE_CATALOG, PIPELINE_SCHEMA, PIPELINE_NAME, SOURCE_SCHEMA, SOURCE_NAME, TEXT_COLUMN, CHUNKING, MODEL_SCHEMA, MODEL_NAME, TARGET_SCHEMA, TARGET_NAME, MODE | vector pipeline of a schema the user owns: its source table and text column, its chunking, its model, its target table, and SYNC or ASYNC *(extension)* ([CREATE VECTOR PIPELINE](statements/create-vector-pipeline.md)) |
| EMBEDDING_QUEUE | TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, MODEL_VERSION, PENDING, EMBEDDED, FAILURES, LAST_ERROR | column the queue after commit fills, of a table of a schema the user owns, or of any to the database's owner: its model's version; the rows waiting as its last round began; those given a vector and the rounds failed since the database opened; the last round's error, or NULL *(extension)* ([CREATE VECTOR PIPELINE](statements/create-vector-pipeline.md)) |
| EXTENSIONS | EXTENSION_NAME, VERSION, DIRECTORY, SIGNER, PROCESSES_RUNNING, PROCESSES_STARTED, PROCESSES_FAILED, CALLS, CALLS_FAILED | extension created: its version, its directory, the key that signed its package or NULL, and of its plug-in since the database opened, the processes running and started, those stopped after a failure, the calls made and those that failed *(extension)* ([CREATE EXTENSION](statements/create-extension.md)) |
| EXTENSION_OBJECTS | EXTENSION_NAME, OBJECT_TYPE, OBJECT_NAME, IS_DETERMINISTIC, BATCH_SIZE, DECLARATION | object an extension's manifest declares: its kind, name, whether it is DETERMINISTIC, its BATCH or NULL, and its declaration as written *(extension)* |
| EXTENSION_PACKAGES | PACKAGE_NAME, PACKAGE_VERSION, DIGEST, REGISTRY, IS_EXPLICIT, REQUIRES, TURBOREL | the database's lock of packages: each extension's version, the SHA-256 digest of its manifest if installed from a package, its registry or NULL, YES if CREATE EXTENSION named it and NO if only required, the ranges it requires of others, and the range of turborel releases it runs on *(extension)* ([Packages](../client/04-plugins-packages.md)) |
| BACKUPS | BACKUP_ID, LOCATION, KIND, PARENT_ID, TIMELINE, START_POSITION, STOP_POSITION, BYTES, STATE, LABEL, STARTED, ENDED | backup of the database's history, to its owner: its id and target, FULL or INCREMENTAL, the backup it follows or NULL, its timeline, where in the log it starts and, once completed, stops, the bytes it wrote, RUNNING, COMPLETED, FAILED, CANCELLED, GONE or INTERRUPTED, its label or NULL, and when it started and ended *(extension)* ([Backups and Recovery](../operator/10-backups.md)) |
| ARCHIVED_SEGMENTS | LOCATION, TIMELINE, FIRST_POSITION, LAST_POSITION, BYTES, ARCHIVED | segment of the log archived and not removed, to the owner: its target, timeline, first and last records' positions, bytes, and when it was archived *(extension)* |
| RESTORE_POINTS | POINT_NAME, POSITION, CREATED | restore point, to the owner: its name, its record's position, and when it was made *(extension)* ([CREATE RESTORE POINT](statements/create-restore-point.md)) |

VALUE_SOURCE says where a value comes from: DEFAULT, or the place it was
set: DATABASE, SESSION, or NODE for a node's setting, whose value is
this node's alone, kept nowhere ([Settings](../appendix/c-settings.md)).
[SET](statements/set-setting.md) and
[SET DATABASE](statements/set-database.md) set the settings;
[Settings](../appendix/c-settings.md) lists them.

```sql
SELECT SETTING_NAME, SETTING_VALUE, VALUE_SOURCE, IS_SESSION_SETTABLE FROM INFORMATION_SCHEMA.SETTINGS
    WHERE SETTING_NAME IN ('LOCKS.WAIT', 'MEMORY.BUDGET_BYTES', 'SESSION.TIME_ZONE');
SELECT * FROM INFORMATION_SCHEMA.MEMORY_RESERVATIONS;
```

```
 SETTING_NAME                                                 | SETTING_VALUE                                                | VALUE_SOURCE | IS_SESSION_SETTABLE
--------------------------------------------------------------+--------------------------------------------------------------+--------------+---------------------
 LOCKS.WAIT                                                   | OFF                                                          | DEFAULT      | YES
 MEMORY.BUDGET_BYTES                                          | 1073741824                                                   | DEFAULT      | NO
 SESSION.TIME_ZONE                                            | +00:00                                                       | DEFAULT      | YES
(3 rows)
 RESERVED_BY                                                  |                BYTES
--------------------------------------------------------------+----------------------
 the buffer pool of the data file                             |            134217728
(1 row)
```

The second, of a database kept in a directory: one in memory has no
buffer pool. MEMORY_RESERVATIONS, FILES, JOBS, CONNECTIONS, SERVER, BACKUPS,
ARCHIVED_SEGMENTS and RESTORE_POINTS show their rows to the database's
owner alone, and a non-owner sees no rows. CONNECTIONS and SERVER are a
server's: a database a server serves, as `turboreld`'s, has them, and
they show no connection, and counts of none, while it does not serve.
JOBS.POLICY says how the jobs JOBS lists are run
([Settings](../appendix/c-settings.md)).

## Graph statistics

GRAPH_STATISTICS *(extension)* shows what the planner knows of each
property graph the user may query. They are made of the statistics of
the graph's tables, as the database's maintenance gathers them, and
are NULL until it has. A row of kind VERTEX or EDGE is of an element
table: its rows and, of an edge table, its degrees. The out-degree of
a vertex is its edges at their source, the in-degree those at their
destination. The average, median, 90th and 99th percentiles and
maximum count each vertex with an edge there once. A row of kind
PROPERTY gives a property's distinct values and its share of NULL, of
a property that is a column alone; of one that is an expression they
are NULL. A row of kind TRANSITION gives the edges from a vertex of one
label along an edge of another to a vertex of a third.

```sql
CREATE TABLE CITY (ID INTEGER PRIMARY KEY, NAME VARCHAR(20));
CREATE TABLE ROAD (ID INTEGER PRIMARY KEY,
    FROM_ID INTEGER NOT NULL REFERENCES CITY, TO_ID INTEGER NOT NULL REFERENCES CITY, KM INTEGER);
INSERT INTO CITY VALUES (1, 'Ankh'), (2, 'Morpork'), (3, 'Quirm'), (4, 'Sto Lat');
INSERT INTO ROAD VALUES (1, 1, 2, 5), (2, 1, 3, 40), (3, 1, 4, 30), (4, 2, 4, 28), (5, 3, 1, NULL);
CREATE PROPERTY GRAPH ROADS
    VERTEX TABLES (CITY LABEL CITY PROPERTIES (ID, NAME))
    EDGE TABLES (ROAD
        SOURCE KEY (FROM_ID) REFERENCES CITY (ID)
        DESTINATION KEY (TO_ID) REFERENCES CITY (ID)
        LABEL ROAD PROPERTIES (KM, KM * 1000 AS METRES));
COMMIT;
```

```
OK
OK
OK
OK
OK
OK
```

Once the maintenance has gathered the statistics of CITY and ROAD:

```sql
SELECT STATISTIC_KIND, ELEMENT_TABLE_ALIAS, ROW_COUNT,
       OUT_DEGREE_AVERAGE, OUT_DEGREE_MAXIMUM, IN_DEGREE_AVERAGE, IN_DEGREE_MAXIMUM
  FROM INFORMATION_SCHEMA.GRAPH_STATISTICS
  WHERE STATISTIC_KIND IN ('VERTEX', 'EDGE') ORDER BY ELEMENT_TABLE_ALIAS;
SELECT ELEMENT_TABLE_ALIAS, PROPERTY_NAME, DISTINCT_VALUES, NULL_FRACTION
  FROM INFORMATION_SCHEMA.GRAPH_STATISTICS
  WHERE STATISTIC_KIND = 'PROPERTY' ORDER BY ELEMENT_TABLE_ALIAS, PROPERTY_NAME;
SELECT SOURCE_LABEL, EDGE_LABEL, DESTINATION_LABEL, ROW_COUNT
  FROM INFORMATION_SCHEMA.GRAPH_STATISTICS WHERE STATISTIC_KIND = 'TRANSITION';
```

```
 STATISTIC_KIND                                               | ELEMENT_TABLE_ALIAS                                          |            ROW_COUNT |   OUT_DEGREE_AVERAGE |   OUT_DEGREE_MAXIMUM |    IN_DEGREE_AVERAGE |    IN_DEGREE_MAXIMUM
--------------------------------------------------------------+--------------------------------------------------------------+----------------------+----------------------+----------------------+----------------------+----------------------
 VERTEX                                                       | CITY                                                         |                 4.00 |                 NULL |                 NULL |                 NULL |                 NULL
 EDGE                                                         | ROAD                                                         |                 5.00 |                 1.67 |                 3.00 |                 1.25 |                 2.00
(2 rows)
 ELEMENT_TABLE_ALIAS                                          | PROPERTY_NAME                                                |      DISTINCT_VALUES |        NULL_FRACTION
--------------------------------------------------------------+--------------------------------------------------------------+----------------------+----------------------
 CITY                                                         | ID                                                           |                 4.00 |                 0.00
 CITY                                                         | NAME                                                         |                 4.00 |                 0.00
 ROAD                                                         | KM                                                           |                 4.00 |                 0.20
 ROAD                                                         | METRES                                                       |                 NULL |                 NULL
(4 rows)
 SOURCE_LABEL                                                 | EDGE_LABEL                                                   | DESTINATION_LABEL                                            |            ROW_COUNT
--------------------------------------------------------------+--------------------------------------------------------------+--------------------------------------------------------------+----------------------
 CITY                                                         | ROAD                                                         | CITY                                                         |                 5.00
(1 row)
```

The planner reads the same statistics. It estimates the rows a probe of an edge table by its endpoint columns
finds as the average degree there, which EXPLAIN shows as `est. rows
each`. A GRAPH_TABLE search whose two end vertices are both filtered
starts from the end of fewer candidate vertices times the fan-out of
its first step from them, its pattern reversed. A search finds the same
paths from either end, so its rows are the same. The hints
GRAPH_START and GRAPH_DIRECTION *(extension)* choose the end instead.
A search whose paths depend on the order it finds them in always starts
from its first vertex. Those are the ANY, ANY SHORTEST, SHORTEST k and
CHEAPEST selectors, and searches under DETERMINISTIC_PATHS,
MAX_PATHS_PER_PARTITION or ON LIMIT TRUNCATE.

```sql
SELECT A, B, KM FROM GRAPH_TABLE (ROADS
    MATCH (a WHERE a.NAME <> 'Quirm')-[r]->{1,2}(b WHERE b.ID = 4)
    COLUMNS (a.NAME AS A, b.NAME AS B, SUM(r.KM) AS KM)
    OPTIONS (GRAPH_START(b)))
  ORDER BY A, KM;
```

```
 A                    | B                    |                   KM
----------------------+----------------------+----------------------
 Ankh                 | Sto Lat              |                   30
 Ankh                 | Sto Lat              |                   33
 Morpork              | Sto Lat              |                   28
(3 rows)
```

The standard: ISO/IEC 9075:1992 §21.1, §21.2; the views of roles and of
triggers, the short name views, and how each view reads the enabled
roles, the 2006 draft of ISO/IEC 9075-11, Clauses 5 and 6; the short
name views of modules and of SQL/XML, the 2006 drafts of ISO/IEC 9075-4,
§18.8, and of ISO/IEC 9075-14, §20.15.
