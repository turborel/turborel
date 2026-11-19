//! The errors XQuery names that analysis of a query in its static context,
//! or its evaluation, raises [XQuery §2.3.1, Appendix F], and those of the
//! functions and operators it maps to [XQueryFO Appendix C].

use super::ErrorCode;

impl ErrorCode {
    /// [XQuery Appendix F] Analysis relies on a component of the static
    /// context that has no value.
    pub const XPST0001: ErrorCode = ErrorCode("XPST0001");
    /// [XQuery Appendix F] The static type of an expression other than
    /// `()` or `data(())` is `empty-sequence()`.
    pub const XPST0005: ErrorCode = ErrorCode("XPST0005");
    /// [XQuery Appendix F] Evaluation relies on a part of the dynamic
    /// context that has no value.
    pub const XPDY0002: ErrorCode = ErrorCode("XPDY0002");
    /// [XQuery Appendix F] The static type of an expression is not
    /// appropriate for the context it is in, or the dynamic type of a
    /// value does not match a required type.
    pub const XPTY0004: ErrorCode = ErrorCode("XPTY0004");
    /// [XQuery Appendix F] A name, prefix or variable is not defined in the
    /// static context.
    pub const XPST0008: ErrorCode = ErrorCode("XPST0008");
    /// [XQuery Appendix F] No function signature has a function call's name
    /// and arity.
    pub const XPST0017: ErrorCode = ErrorCode("XPST0017");
    /// [XQuery Appendix F] The last step of a path gives both nodes and
    /// atomic values.
    pub const XPTY0018: ErrorCode = ErrorCode("XPTY0018");
    /// [XQuery Appendix F] A step other than the last gives an atomic
    /// value.
    pub const XPTY0019: ErrorCode = ErrorCode("XPTY0019");
    /// [XQuery Appendix F] The context item of an axis step is no node.
    pub const XPTY0020: ErrorCode = ErrorCode("XPTY0020");
    /// [XQuery Appendix F] An element constructor's content has an
    /// attribute after a node that is not one.
    pub const XQTY0024: ErrorCode = ErrorCode("XQTY0024");
    /// [XQuery Appendix F] Two attributes of a constructed element have one
    /// name.
    pub const XQDY0025: ErrorCode = ErrorCode("XQDY0025");
    /// [XQuery Appendix F] A computed processing instruction's content
    /// holds "?>".
    pub const XQDY0026: ErrorCode = ErrorCode("XQDY0026");
    /// [XQuery Appendix F] The validity property of the root element of a
    /// validate expression's operand is not valid, nor notKnown in lax
    /// mode.
    pub const XQDY0027: ErrorCode = ErrorCode("XQDY0027");
    /// [XQuery Appendix F] A validate expression's operand is not one
    /// document or element node.
    pub const XQTY0030: ErrorCode = ErrorCode("XQTY0030");
    /// [XQuery Appendix F] Two functions of a module have one expanded
    /// QName and arity.
    pub const XQST0034: ErrorCode = ErrorCode("XQST0034");
    /// [XQuery Appendix F] A computed processing instruction's name is no
    /// `xs:NCName`.
    pub const XQDY0041: ErrorCode = ErrorCode("XQDY0041");
    /// [XQuery Appendix F] A computed attribute is named in the namespace
    /// of `xmlns`, or `xmlns` in no namespace.
    pub const XQDY0044: ErrorCode = ErrorCode("XQDY0044");
    /// [XQuery Appendix F] A module imported declares a variable or
    /// function of a schema type the importing module's in-scope schema
    /// types do not include.
    pub const XQST0036: ErrorCode = ErrorCode("XQST0036");
    /// [XQuery Appendix F] A function is declared in a namespace reserved
    /// to XML, XML Schema or XQuery.
    pub const XQST0045: ErrorCode = ErrorCode("XQST0045");
    /// [XQuery Appendix F] Two module imports of a prolog specify one target
    /// namespace.
    pub const XQST0047: ErrorCode = ErrorCode("XQST0047");
    /// [XQuery Appendix F] A function or variable a library module declares
    /// is not in its target namespace.
    pub const XQST0048: ErrorCode = ErrorCode("XQST0048");
    /// [XQuery Appendix F] Two variables of a module have one expanded
    /// QName.
    pub const XQST0049: ErrorCode = ErrorCode("XQST0049");
    /// [XQuery Appendix F] A treat expression's operand does not match its
    /// type, or a leading "/" finds no document node.
    pub const XPDY0050: ErrorCode = ErrorCode("XPDY0050");
    /// [XQuery Appendix F] An atomic type of a sequence type is no atomic
    /// type in the in-scope schema types.
    pub const XPST0051: ErrorCode = ErrorCode("XPST0051");
    /// [XQuery Appendix F] A variable's initializing expression depends on
    /// the variable.
    pub const XQST0054: ErrorCode = ErrorCode("XQST0054");
    /// [XQuery Appendix F] A function is declared in no namespace.
    pub const XQST0060: ErrorCode = ErrorCode("XQST0060");
    /// [XQuery Appendix F] A validate expression's document node does not
    /// have exactly one element child among comments and processing
    /// instructions.
    pub const XQDY0061: ErrorCode = ErrorCode("XQDY0061");
    /// [XQuery Appendix F] A computed processing instruction is named
    /// "XML" in any case.
    pub const XQDY0064: ErrorCode = ErrorCode("XQDY0064");
    /// [XQuery Appendix F] A computed comment holds two adjacent hyphens
    /// or ends with one.
    pub const XQDY0072: ErrorCode = ErrorCode("XQDY0072");
    /// [XQuery Appendix F] The graph of module imports has a cycle of
    /// modules not all of one namespace.
    pub const XQST0073: ErrorCode = ErrorCode("XQST0073");
    /// [XQuery Appendix F] A computed element or attribute's name is no
    /// expanded QName.
    pub const XQDY0074: ErrorCode = ErrorCode("XQDY0074");
    /// [XQuery Appendix F] An order by clause names a collation not
    /// statically known.
    pub const XQST0076: ErrorCode = ErrorCode("XQST0076");
    /// [XQuery Appendix F] An extension expression has no pragma the
    /// implementation recognizes and no expression in braces.
    pub const XQST0079: ErrorCode = ErrorCode("XQST0079");
    /// [XQuery Appendix F] A cast's target type is no atomic type, or is
    /// `xs:NOTATION` or `xdt:anyAtomicType`.
    pub const XPST0080: ErrorCode = ErrorCode("XPST0080");
    /// [XQuery Appendix F] A QName's prefix is bound by no statically
    /// known namespace.
    pub const XPST0081: ErrorCode = ErrorCode("XPST0081");
    /// [XQuery Appendix F] A cast to `xs:QName` or `xs:NOTATION` is of no
    /// string literal.
    pub const XPST0083: ErrorCode = ErrorCode("XPST0083");
    /// [XQuery Appendix F] Strict validation finds no top-level element
    /// declaration.
    pub const XQDY0084: ErrorCode = ErrorCode("XQDY0084");
    /// [XQueryFO Appendix C] Division by zero.
    pub const FOAR0001: ErrorCode = ErrorCode("FOAR0001");
    /// [XQueryFO Appendix C] Numeric operation overflow or underflow.
    pub const FOAR0002: ErrorCode = ErrorCode("FOAR0002");
    /// [XQueryFO Appendix C] NaN supplied as a float or double value.
    pub const FOCA0005: ErrorCode = ErrorCode("FOCA0005");
    /// [XQueryFO Appendix C] Unsupported collation.
    pub const FOCH0002: ErrorCode = ErrorCode("FOCH0002");
    /// [XQueryFO Appendix C] Invalid argument to fn:doc.
    pub const FODC0005: ErrorCode = ErrorCode("FODC0005");
    /// [XQueryFO Appendix C] Overflow or underflow in a date or time
    /// operation.
    pub const FODT0001: ErrorCode = ErrorCode("FODT0001");
    /// [XQueryFO Appendix C] Overflow or underflow in a duration
    /// operation.
    pub const FODT0002: ErrorCode = ErrorCode("FODT0002");
    /// [XQueryFO Appendix C] Undefined context item.
    pub const FONC0001: ErrorCode = ErrorCode("FONC0001");
    /// [XQueryFO Appendix C] Invalid value for a cast or constructor.
    pub const FORG0001: ErrorCode = ErrorCode("FORG0001");
    /// [XQueryFO Appendix C] Invalid argument type.
    pub const FORG0006: ErrorCode = ErrorCode("FORG0006");
    /// [XQueryFO Appendix C] The context item is not a node.
    pub const FOTY0011: ErrorCode = ErrorCode("FOTY0011");

    /// [XQueryFO Appendix C] Invalid lexical value.
    pub const FOCA0002: ErrorCode = ErrorCode("FOCA0002");
    /// [XQueryFO Appendix C] Code point not valid.
    pub const FOCH0001: ErrorCode = ErrorCode("FOCH0001");
    /// [XQueryFO Appendix C] Unsupported normalization form.
    pub const FOCH0003: ErrorCode = ErrorCode("FOCH0003");
    /// [XQueryFO Appendix C] Collation does not support collation units.
    pub const FOCH0004: ErrorCode = ErrorCode("FOCH0004");
    /// [XQueryFO Appendix C] No context document.
    pub const FODC0001: ErrorCode = ErrorCode("FODC0001");
    /// [XQueryFO Appendix C] Error retrieving resource.
    pub const FODC0002: ErrorCode = ErrorCode("FODC0002");
    /// [XQueryFO Appendix C] Invalid argument to fn:collection().
    pub const FODC0004: ErrorCode = ErrorCode("FODC0004");
    /// [XQueryFO Appendix C] Invalid timezone value.
    pub const FODT0003: ErrorCode = ErrorCode("FODT0003");
    /// [XQueryFO Appendix C] Unidentified error.
    pub const FOER0000: ErrorCode = ErrorCode("FOER0000");
    /// [XQueryFO Appendix C] No namespace found for prefix.
    pub const FONS0004: ErrorCode = ErrorCode("FONS0004");
    /// [XQueryFO Appendix C] Base-uri not defined in the static context.
    pub const FONS0005: ErrorCode = ErrorCode("FONS0005");
    /// [XQueryFO Appendix C] fn:zero-or-one called with a sequence
    /// containing more than one item.
    pub const FORG0003: ErrorCode = ErrorCode("FORG0003");
    /// [XQueryFO Appendix C] fn:one-or-more called with a sequence containing
    /// no items.
    pub const FORG0004: ErrorCode = ErrorCode("FORG0004");
    /// [XQueryFO Appendix C] fn:exactly-one called with a sequence containing
    /// zero or more than one item.
    pub const FORG0005: ErrorCode = ErrorCode("FORG0005");
    /// [XQueryFO Appendix C] Both arguments to fn:dateTime have a specified
    /// timezone.
    pub const FORG0008: ErrorCode = ErrorCode("FORG0008");
    /// [XQueryFO Appendix C] Error in resolving a relative URI against a base
    /// URI in fn:resolve-uri.
    pub const FORG0009: ErrorCode = ErrorCode("FORG0009");
    /// [XQueryFO Appendix C] Invalid regular expression flags.
    pub const FORX0001: ErrorCode = ErrorCode("FORX0001");
    /// [XQueryFO Appendix C] Invalid regular expression.
    pub const FORX0002: ErrorCode = ErrorCode("FORX0002");
    /// [XQueryFO Appendix C] Regular expression matches zero-length string.
    pub const FORX0003: ErrorCode = ErrorCode("FORX0003");
    /// [XQueryFO Appendix C] Invalid replacement string.
    pub const FORX0004: ErrorCode = ErrorCode("FORX0004");

    /// The code `local`, the local part of an error's QName in the
    /// namespace of \[XQuery\]'s and \[XQueryFO\]'s errors [XQuery §2.3.2],
    /// for an error a function of a plug-in raises (`XQueryFunction`).
    #[must_use]
    pub const fn new(local: &'static str) -> ErrorCode {
        ErrorCode(local)
    }
}
