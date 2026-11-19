//! [ISO/IEC 9075:1992 §21.3.5] A data type as the Information Schema
//! describes it (`Descriptor`), with what an XML type of XMLSCHEMA names.

/// [ISO/IEC 9075:1992 §21.3.5] A data type as the Information Schema
/// describes it: DATA_TYPE and the columns that apply to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Descriptor {
    /// DATA_TYPE: its name.
    pub name: String,
    /// CHARACTER_MAXIMUM_LENGTH, of a character string.
    pub length: Option<u64>,
    /// CHARACTER_OCTET_LENGTH, of a character string.
    pub octets: Option<u64>,
    /// Whether it has a collation, SQL_TEXT's: a character string.
    pub collated: bool,
    /// NUMERIC_PRECISION, of a number.
    pub precision: Option<u32>,
    /// NUMERIC_PRECISION_RADIX: 10 for decimal precision, 2 for binary.
    pub radix: Option<u32>,
    /// NUMERIC_SCALE, of an exact number.
    pub scale: Option<u32>,
    /// DATETIME_PRECISION, of a datetime or interval.
    pub datetime_precision: Option<u32>,
    /// MAXIMUM_CARDINALITY, of an array [ISO/IEC 9075-2:2023 §4.12.1].
    pub cardinality: Option<u32>,
    /// USER_DEFINED_TYPE_SCHEMA and USER_DEFINED_TYPE_NAME, of a
    /// user-defined type [ISO/IEC 9075-2:2023 §4.9.2].
    pub user_defined: Option<(String, String)>,
    /// XML_PRIMARY_MODIFIER and XML_SECONDARY_MODIFIER, of an XML type
    /// [ISO/IEC 9075-14:2006 draft §21.1 Description 1a].
    pub xml: Option<(&'static str, Option<&'static str>)>,
    /// Of an XML type of XMLSCHEMA, its registered XML Schema, indicated
    /// XML namespace and global element declaration [ISO/IEC 9075-14:2006
    /// draft §6.1 GR1d to GR1f, §21.1 Description 1a iii 1].
    pub xml_schema: Option<XmlSchemaNamed>,
}

/// What an XML type of XMLSCHEMA names [ISO/IEC 9075-14:2006 draft §6.1
/// GR1d to GR1f], as XML_SCHEMA_SCHEMA, XML_SCHEMA_NAME,
/// XML_SCHEMA_NAMESPACE and XML_SCHEMA_ELEMENT show it [§21.1
/// Description 1a iii 1].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlSchemaNamed {
    /// The schema of its registered XML Schema.
    pub schema: String,
    /// The name of its registered XML Schema, in that schema.
    pub name: String,
    /// The URI of its indicated XML namespace, if any.
    pub namespace: Option<String>,
    /// The NCName of its indicated global element declaration, if any.
    pub element: Option<String>,
}

impl Descriptor {
    /// A descriptor of `name`, every other column null.
    pub fn of(name: &str) -> Descriptor {
        Descriptor {
            name: name.to_owned(),
            length: None,
            octets: None,
            collated: false,
            precision: None,
            radix: None,
            scale: None,
            datetime_precision: None,
            cardinality: None,
            user_defined: None,
            xml: None,
            xml_schema: None,
        }
    }
}
