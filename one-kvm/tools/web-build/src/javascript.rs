//! All source ranges originate in Oxc's AST, never a JavaScript text regex.
use anyhow::{Result, bail, ensure};
use oxc::{
    allocator::Allocator,
    ast::ast::*,
    ast_visit::{Visit, walk},
    codegen::{Codegen, CodegenOptions},
    minifier::{Minifier, MinifierOptions},
    parser::Parser,
    semantic::SemanticBuilder,
    span::{SourceType, Span},
};

#[derive(Debug)]
pub struct Import {
    pub url: String,
    pub source: Span,
    pub statement: Span,
    pub asset_binding: Option<String>,
    pub is_export: bool,
    pub has_bindings: bool,
}

#[derive(Debug, Default)]
pub struct References {
    pub imports: Vec<Import>,
    pub assets: Vec<(String, Span)>,
    pub dynamic: Vec<(String, Span)>,
    errors: Vec<String>,
}

impl<'a> Visit<'a> for References {
    fn visit_import_declaration(&mut self, node: &ImportDeclaration<'a>) {
        let asset_binding = node.specifiers.as_ref().and_then(|items| {
            if items.len() == 1 {
                match &items[0] {
                    ImportDeclarationSpecifier::ImportDefaultSpecifier(item) => {
                        Some(item.local.name.to_string())
                    }
                    _ => None,
                }
            } else {
                None
            }
        });
        if node.with_clause.is_some() || node.phase.is_some() {
            self.errors
                .push("Import attributes/phases are unsupported".into());
        }
        self.imports.push(Import {
            url: node.source.value.to_string(),
            source: node.source.span,
            statement: node.span,
            asset_binding,
            is_export: false,
            has_bindings: node
                .specifiers
                .as_ref()
                .is_some_and(|items| !items.is_empty()),
        });
    }
    fn visit_export_from_declaration(&mut self, node: &ExportFromDeclaration<'a>) {
        let binding = if node.specifiers.len() == 1
            && node.specifiers[0].local.name().as_str() == "default"
        {
            Some(node.specifiers[0].exported.name().to_string())
        } else {
            None
        };
        self.imports.push(Import {
            url: node.source.value.to_string(),
            source: node.source.span,
            statement: node.span,
            asset_binding: binding,
            is_export: true,
            has_bindings: true,
        });
    }
    fn visit_export_all_declaration(&mut self, node: &ExportAllDeclaration<'a>) {
        self.imports.push(Import {
            url: node.source.value.to_string(),
            source: node.source.span,
            statement: node.span,
            asset_binding: None,
            is_export: true,
            has_bindings: true,
        });
    }
    fn visit_import_expression(&mut self, node: &ImportExpression<'a>) {
        if let Expression::StringLiteral(literal) = &node.source {
            if node.options.is_none() && node.phase.is_none() {
                self.dynamic.push((literal.value.to_string(), literal.span));
            } else {
                self.errors
                    .push("Dynamic import attributes are unsupported".into());
            }
        } else {
            self.errors.push(
                "Computed dynamic imports are unsupported; use an explicit vendored ESM import"
                    .into(),
            );
        }
        walk::walk_import_expression(self, node);
    }
    fn visit_new_expression(&mut self, node: &NewExpression<'a>) {
        if matches!(&node.callee, Expression::Identifier(name) if name.name == "URL")
            && node.arguments.len() == 2
            && let (Argument::StringLiteral(literal), Argument::StaticMemberExpression(member)) =
                (&node.arguments[0], &node.arguments[1])
            && member.property.name == "url"
            && matches!(&member.object, Expression::ImportMeta(_))
        {
            self.assets.push((literal.value.to_string(), literal.span));
        }
        walk::walk_new_expression(self, node);
    }
    fn visit_call_expression(&mut self, node: &CallExpression<'a>) {
        if matches!(&node.callee, Expression::Identifier(name) if name.name == "require") {
            self.errors.push(
                "CommonJS require() is unsupported; vendor the upstream ESM distribution".into(),
            );
        }
        walk::walk_call_expression(self, node);
    }
}

pub fn references(text: &str) -> Result<References> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, SourceType::mjs()).parse();
    ensure!(
        parsed.diagnostics.is_empty(),
        "JavaScript syntax error: {:?}",
        parsed.diagnostics
    );
    let semantic = SemanticBuilder::new()
        .with_check_syntax_error(true)
        .build(&parsed.program);
    ensure!(
        semantic.diagnostics.is_empty(),
        "JavaScript semantic error: {:?}",
        semantic.diagnostics
    );
    let mut references = References::default();
    references.visit_program(&parsed.program);
    if !references.errors.is_empty() {
        bail!("{}", references.errors.join("; "));
    }
    Ok(references)
}

pub fn minify(text: &str) -> Result<Vec<u8>> {
    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, text, SourceType::mjs()).parse();
    ensure!(
        parsed.diagnostics.is_empty(),
        "Linked JavaScript syntax error: {:?}",
        parsed.diagnostics
    );
    let mut program = parsed.program;
    let result = Minifier::new(MinifierOptions::default()).minify(&allocator, &mut program);
    Ok(Codegen::new()
        .with_options(CodegenOptions::minify())
        .with_scoping(result.scoping)
        .build(&program)
        .code
        .into_bytes())
}

pub fn range(span: Span) -> (usize, usize) {
    (span.start as usize, span.end as usize)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strings_comments_regex_are_not_module_edges() -> Result<()> {
        let text = r#"// import x from 'fake';
const s="import x from 'fake'"; const r=/import\("fake"\)/;
import {x} from "./real.js"; export { x as y } from './other.js';
const asset=new URL('./a.wasm',import.meta.url);"#;
        let result = references(text)?;
        assert_eq!(result.imports.len(), 2);
        assert_eq!(result.assets.len(), 1);
        assert_eq!(result.imports[0].url, "./real.js");
        assert!(references("import(window.name)").is_err());
        Ok(())
    }
    #[test]
    fn compression_preserves_string_and_regular_expression_literals() -> Result<()> {
        let code = minify("export const s = 'a  b'; export const r = /a  b/; // comment\n")?;
        let result = String::from_utf8(code)?;
        assert!(result.contains("a  b"));
        assert!(!result.contains("// comment"));
        references(&result)?;
        Ok(())
    }
}
