use std::collections::{BTreeSet, HashMap};

use serde_json::Value;
use swc_common::sync::Lrc;
use swc_common::{FileName, SourceMap};
use swc_ecma_ast::{
    Decl, Expr, ModuleDecl, ModuleItem, Stmt, TsInterfaceDecl, TsKeywordTypeKind, TsLit, TsType,
    TsTypeElement, TsTypeOperatorOp, TsUnionOrIntersectionType,
};
use swc_ecma_parser::{Parser, StringInput, Syntax, TsSyntax};

pub struct Api {
    interfaces: HashMap<String, TsInterfaceDecl>,
    aliases: HashMap<String, TsType>,
}

impl Api {
    pub fn load() -> Api {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../api/aqx.d.ts");
        let source = std::fs::read_to_string(path).unwrap();
        let map: Lrc<SourceMap> = Lrc::default();
        let file = map.new_source_file(Lrc::new(FileName::Custom(path.into())), source);
        let syntax = Syntax::Typescript(TsSyntax {
            dts: true,
            ..TsSyntax::default()
        });
        let module = Parser::new(syntax, StringInput::from(&*file), None)
            .parse_module()
            .unwrap();
        let mut api = Api {
            interfaces: HashMap::new(),
            aliases: HashMap::new(),
        };
        for item in module.body {
            let decl = match item {
                ModuleItem::Stmt(Stmt::Decl(decl)) => decl,
                ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => export.decl,
                _ => continue,
            };
            match decl {
                Decl::TsInterface(interface) => {
                    api.interfaces
                        .insert(interface.id.sym.to_string(), *interface);
                }
                Decl::TsTypeAlias(alias) => {
                    api.aliases
                        .insert(alias.id.sym.to_string(), *alias.type_ann);
                }
                _ => {}
            }
        }
        api
    }

    pub fn check(&self, name: &str, value: &Value) -> Result<(), String> {
        let ty = self.named(name);
        self.check_type(&ty, value, name)
    }

    pub fn check_event(&self, name: &str, value: &Value) -> Result<(), String> {
        let ty = self
            .properties_of("Events")
            .remove(name)
            .ok_or_else(|| format!("Events has no {name}"))?;
        self.check_type(ty, value, &format!("Events.{name}"))
    }

    fn named(&self, name: &str) -> TsType {
        let source = format!("type T = {name};");
        let map: Lrc<SourceMap> = Lrc::default();
        let file = map.new_source_file(Lrc::new(FileName::Anon), source);
        let module = Parser::new(
            Syntax::Typescript(TsSyntax::default()),
            StringInput::from(&*file),
            None,
        )
        .parse_module()
        .unwrap();
        match module.body.into_iter().next() {
            Some(ModuleItem::Stmt(Stmt::Decl(Decl::TsTypeAlias(alias)))) => *alias.type_ann,
            _ => unreachable!("a type alias parses as one"),
        }
    }

    fn check_type(&self, ty: &TsType, value: &Value, path: &str) -> Result<(), String> {
        let mismatch = || Err(format!("{path}: {value} doesn't fit the type"));
        match ty {
            TsType::TsKeywordType(keyword) => match (keyword.kind, value) {
                (TsKeywordTypeKind::TsNumberKeyword, Value::Number(_))
                | (TsKeywordTypeKind::TsStringKeyword, Value::String(_))
                | (TsKeywordTypeKind::TsBooleanKeyword, Value::Bool(_))
                | (TsKeywordTypeKind::TsNullKeyword, Value::Null)
                | (TsKeywordTypeKind::TsUnknownKeyword, _) => Ok(()),
                _ => mismatch(),
            },
            TsType::TsLitType(literal) => match (&literal.lit, value) {
                (TsLit::Str(text), Value::String(found)) if text.value == **found => Ok(()),
                (TsLit::Number(number), Value::Number(found))
                    if found.as_f64() == Some(number.value) =>
                {
                    Ok(())
                }
                (TsLit::Bool(flag), Value::Bool(found)) if flag.value == *found => Ok(()),
                _ => mismatch(),
            },
            TsType::TsParenthesizedType(inner) => self.check_type(&inner.type_ann, value, path),
            TsType::TsTypeOperator(operator) if operator.op == TsTypeOperatorOp::ReadOnly => {
                self.check_type(&operator.type_ann, value, path)
            }
            TsType::TsArrayType(array) => {
                let Value::Array(items) = value else {
                    return mismatch();
                };
                items.iter().enumerate().try_for_each(|(i, item)| {
                    self.check_type(&array.elem_type, item, &format!("{path}[{i}]"))
                })
            }
            TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsUnionType(union)) => {
                let mut tried: Vec<String> = union
                    .types
                    .iter()
                    .filter_map(|member| self.check_type(member, value, path).err())
                    .collect();
                let fits = tried.len() < union.types.len();
                tried.sort();
                tried.dedup();
                if fits {
                    Ok(())
                } else {
                    Err(tried.join("; or "))
                }
            }
            TsType::TsTypeRef(reference) => {
                let name = reference.type_name.as_ident().map(|i| i.sym.to_string());
                match name.as_deref().and_then(|name| self.aliases.get(name)) {
                    Some(alias) => self.check_type(alias, value, path),
                    None => self.check_object(ty, value, path),
                }
            }
            TsType::TsIndexedAccessType(access) => {
                let TsType::TsLitType(key) = &*access.index_type else {
                    return Err(format!("{path}: unsupported index"));
                };
                let TsLit::Str(key) = &key.lit else {
                    return Err(format!("{path}: unsupported index"));
                };
                let key = key.value.to_string_lossy();
                let property = self
                    .properties(&access.obj_type)
                    .remove(key.as_ref())
                    .ok_or_else(|| format!("{path}: no property {key}"))?;
                self.check_type(property, value, path)
            }
            TsType::TsTypeLit(_)
            | TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsIntersectionType(_)) => {
                self.check_object(ty, value, path)
            }
            _ => Err(format!("{path}: the checker doesn't read this type")),
        }
    }

    fn check_object(&self, ty: &TsType, value: &Value, path: &str) -> Result<(), String> {
        let Value::Object(object) = value else {
            return Err(format!("{path}: {value} isn't an object"));
        };
        let properties = self.properties(ty);
        let declared: BTreeSet<&str> = properties.keys().map(String::as_str).collect();
        let built: BTreeSet<&str> = object.keys().map(String::as_str).collect();
        if declared != built {
            return Err(format!(
                "{path}: the types declare {declared:?}, but aqx builds {built:?}"
            ));
        }
        properties
            .iter()
            .try_for_each(|(key, ty)| self.check_type(ty, &object[key], &format!("{path}.{key}")))
    }

    fn properties<'a>(&'a self, ty: &'a TsType) -> HashMap<String, &'a TsType> {
        let mut properties = HashMap::new();
        match ty {
            TsType::TsParenthesizedType(inner) => return self.properties(&inner.type_ann),
            TsType::TsTypeLit(literal) => collect(&literal.members, &mut properties),
            TsType::TsUnionOrIntersectionType(TsUnionOrIntersectionType::TsIntersectionType(
                both,
            )) => {
                for member in &both.types {
                    properties.extend(self.properties(member));
                }
            }
            TsType::TsTypeRef(reference) => {
                if let Some(name) = reference.type_name.as_ident() {
                    return match self.aliases.get(&*name.sym) {
                        Some(alias) => self.properties(alias),
                        None => self.properties_of(&name.sym),
                    };
                }
            }
            _ => {}
        }
        properties
    }

    fn properties_of(&self, name: &str) -> HashMap<String, &TsType> {
        let Some(interface) = self.interfaces.get(name) else {
            return HashMap::new();
        };
        let mut properties = HashMap::new();
        for parent in &interface.extends {
            if let Expr::Ident(parent) = &*parent.expr {
                properties.extend(self.properties_of(&parent.sym));
            }
        }
        collect(&interface.body.body, &mut properties);
        properties
    }
}

fn collect<'a>(members: &'a [TsTypeElement], into: &mut HashMap<String, &'a TsType>) {
    for member in members {
        let TsTypeElement::TsPropertySignature(property) = member else {
            continue;
        };
        let Some(ty) = &property.type_ann else {
            continue;
        };
        let key = match &*property.key {
            Expr::Ident(ident) => ident.sym.to_string(),
            _ => continue,
        };
        into.insert(key, &ty.type_ann);
    }
}
