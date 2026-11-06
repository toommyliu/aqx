use serde_json::{Map, Value};
use swc_common::sync::Lrc;
use swc_common::{FileName, SourceMap};
use swc_ecma_ast::{
    Callee, Expr, ImportSpecifier, MemberProp, Module, ModuleDecl, ModuleExportName, ModuleItem,
    TsEntityName, TsKeywordTypeKind, TsType, TsTypeElement, TsTypeOperatorOp,
    TsUnionOrIntersectionType,
};
use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax};
use swc_ecma_visit::{Visit, VisitWith};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arg {
    pub name: String,
    pub kind: ArgKind,
    pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scalar {
    String,
    Number,
    Boolean,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgKind {
    Scalar(Scalar),
    List(Scalar),
    Map(Scalar),
}

impl Arg {
    pub fn parse(&self, text: &str) -> Result<Value, String> {
        let value = match self.kind {
            ArgKind::Scalar(Scalar::String) => return Ok(Value::String(text.to_owned())),
            ArgKind::Scalar(scalar) => scalar_of(scalar, text),
            ArgKind::List(scalar) => match serde_json::from_str(text) {
                Ok(Value::Array(items)) if items.iter().all(|item| fits(scalar, item)) => {
                    Some(Value::Array(items))
                }
                _ => None,
            },
            ArgKind::Map(scalar) => match serde_json::from_str(text) {
                Ok(Value::Object(map)) if map.values().all(|item| fits(scalar, item)) => {
                    Some(Value::Object(map))
                }
                _ => None,
            },
        };
        value.ok_or_else(|| format!("{} must be {}, not {text}", self.name, self.kind.shown()))
    }
}

impl ArgKind {
    fn shown(self) -> String {
        match self {
            ArgKind::Scalar(scalar) => scalar.shown().to_owned(),
            ArgKind::List(scalar) => format!("a JSON list of {}s", scalar.shown()),
            ArgKind::Map(scalar) => format!("a JSON object of {}s", scalar.shown()),
        }
    }
}

impl Scalar {
    fn shown(self) -> &'static str {
        match self {
            Scalar::String => "string",
            Scalar::Number => "number",
            Scalar::Boolean => "true or false",
        }
    }
}

fn scalar_of(scalar: Scalar, text: &str) -> Option<Value> {
    match scalar {
        Scalar::String => Some(Value::String(text.to_owned())),
        Scalar::Number => text
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .and_then(serde_json::Number::from_f64)
            .map(Value::Number),
        Scalar::Boolean => match text {
            "true" => Some(Value::Bool(true)),
            "false" => Some(Value::Bool(false)),
            _ => None,
        },
    }
}

fn fits(scalar: Scalar, value: &Value) -> bool {
    matches!(
        (scalar, value),
        (Scalar::String, Value::String(_))
            | (Scalar::Number, Value::Number(_))
            | (Scalar::Boolean, Value::Bool(_))
    )
}

pub fn parse_all(args: &[Arg], given: &[(String, String)]) -> Result<Map<String, Value>, String> {
    let mut values = Map::new();
    for (name, text) in given {
        let Some(arg) = args.iter().find(|arg| arg.name == *name) else {
            let known: Vec<&str> = args.iter().map(|arg| arg.name.as_str()).collect();
            return Err(if known.is_empty() {
                format!("the script takes no args, so --arg {name} can't apply")
            } else {
                format!(
                    "the script has no arg {name}; its args are {}",
                    known.join(", ")
                )
            });
        };
        values.insert(name.clone(), arg.parse(text)?);
    }
    let missing: Vec<&str> = args
        .iter()
        .filter(|arg| !arg.optional && !values.contains_key(&arg.name))
        .map(|arg| arg.name.as_str())
        .collect();
    if !missing.is_empty() {
        return Err(format!("the script needs --arg for {}", missing.join(", ")));
    }
    Ok(values)
}

pub fn schema(name: &str, source: &str) -> Result<Vec<Arg>, String> {
    let map: Lrc<SourceMap> = Lrc::default();
    let file = map.new_source_file(
        Lrc::new(FileName::Custom(name.to_owned())),
        source.to_owned(),
    );
    let syntax = Syntax::Typescript(TsSyntax::default());
    let module = Parser::new(syntax, StringInput::from(&*file), None)
        .parse_module()
        .map_err(|e| format!("{name}: {:?}", e.kind()))?;
    let Some(local) = script_import(&module) else {
        return Ok(Vec::new());
    };
    let mut finder = Finder { local, found: None };
    module.visit_with(&mut finder);
    finder.found.unwrap_or(Ok(Vec::new()))
}

fn script_import(module: &Module) -> Option<String> {
    module.body.iter().find_map(|item| {
        let ModuleItem::ModuleDecl(ModuleDecl::Import(import)) = item else {
            return None;
        };
        if import.src.value != *"mira" {
            return None;
        }
        import.specifiers.iter().find_map(|specifier| {
            let ImportSpecifier::Named(named) = specifier else {
                return None;
            };
            let imported = match &named.imported {
                Some(ModuleExportName::Ident(ident)) => ident.sym.to_string(),
                Some(ModuleExportName::Str(name)) => name.value.to_string_lossy().into_owned(),
                None => named.local.sym.to_string(),
            };
            (imported == "script").then(|| named.local.sym.to_string())
        })
    })
}

struct Finder {
    local: String,
    found: Option<Result<Vec<Arg>, String>>,
}

impl Visit for Finder {
    fn visit_call_expr(&mut self, call: &swc_ecma_ast::CallExpr) {
        call.visit_children_with(self);
        if self.found.is_some() {
            return;
        }
        let Callee::Expr(callee) = &call.callee else {
            return;
        };
        let Expr::Member(member) = &**callee else {
            return;
        };
        let called_args = matches!(&member.prop, MemberProp::Ident(prop) if prop.sym == *"args");
        let on_script = matches!(&*member.obj, Expr::Ident(obj) if obj.sym == *self.local);
        if !called_args || !on_script {
            return;
        }
        let Some(params) = &call.type_args else {
            self.found = Some(Ok(Vec::new()));
            return;
        };
        self.found = Some(match params.params.first().map(|t| &**t) {
            Some(TsType::TsTypeLit(literal)) => literal.members.iter().map(arg_of).collect(),
            _ => Err("script.args needs its type written inline, as script.args<{ ... }>()".into()),
        });
    }
}

fn arg_of(member: &TsTypeElement) -> Result<Arg, String> {
    let TsTypeElement::TsPropertySignature(property) = member else {
        return Err("script.args's type can only list properties".into());
    };
    let name = match &*property.key {
        Expr::Ident(ident) => ident.sym.to_string(),
        Expr::Lit(swc_ecma_ast::Lit::Str(text)) => text.value.to_string_lossy().into_owned(),
        _ => return Err("script.args's keys must be plain names".into()),
    };
    let unreadable = || {
        format!(
            "script.args can't read the type of {name}; use string, number, boolean, \
             lists of them, or Record<string, ...> of them"
        )
    };
    let ty = property.type_ann.as_ref().ok_or_else(unreadable)?;
    let (kind, maybe_undefined) = kind_of(&ty.type_ann).ok_or_else(unreadable)?;
    Ok(Arg {
        name,
        kind,
        optional: property.optional || maybe_undefined,
    })
}

fn kind_of(ty: &TsType) -> Option<(ArgKind, bool)> {
    match ty {
        TsType::TsParenthesizedType(inner) => kind_of(&inner.type_ann),
        TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(union)) => {
            let (undefined, rest): (Vec<&TsType>, Vec<&TsType>) =
                union.types.iter().map(|t| &**t).partition(|t| {
                    matches!(t, TsType::TsKeywordType(k) if k.kind == TsKeywordTypeKind::TsUndefinedKeyword)
                });
            match rest.as_slice() {
                [only] if !undefined.is_empty() => kind_of(only).map(|(kind, _)| (kind, true)),
                _ => None,
            }
        }
        TsType::TsTypeOperator(operator) if operator.op == TsTypeOperatorOp::ReadOnly => {
            kind_of(&operator.type_ann)
        }
        TsType::TsArrayType(array) => scalar(&array.elem_type).map(|s| (ArgKind::List(s), false)),
        TsType::TsTypeRef(reference) => {
            let TsEntityName::Ident(name) = &reference.type_name else {
                return None;
            };
            let params: Vec<&TsType> = reference
                .type_params
                .iter()
                .flat_map(|p| p.params.iter().map(|t| &**t))
                .collect();
            match (&*name.sym, params.as_slice()) {
                ("Readonly", [inner]) => kind_of(inner),
                ("Array" | "ReadonlyArray", [inner]) => {
                    scalar(inner).map(|s| (ArgKind::List(s), false))
                }
                ("Record", [key, value]) if scalar(key) == Some(Scalar::String) => {
                    scalar(value).map(|s| (ArgKind::Map(s), false))
                }
                _ => None,
            }
        }
        _ => scalar(ty).map(|s| (ArgKind::Scalar(s), false)),
    }
}

fn scalar(ty: &TsType) -> Option<Scalar> {
    let TsType::TsKeywordType(keyword) = ty else {
        return None;
    };
    match keyword.kind {
        TsKeywordTypeKind::TsStringKeyword => Some(Scalar::String),
        TsKeywordTypeKind::TsNumberKeyword => Some(Scalar::Number),
        TsKeywordTypeKind::TsBooleanKeyword => Some(Scalar::Boolean),
        _ => None,
    }
}
