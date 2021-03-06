//! This module generates AST datatype used by rust-analyzer.
//!
//! Specifically, it generates the `SyntaxKind` enum and a number of newtype
//! wrappers around `SyntaxNode` which implement `ra_syntax::AstNode`.

use std::{
    borrow::Cow,
    collections::{BTreeSet, HashSet},
};

use proc_macro2::{Punct, Spacing};
use quote::{format_ident, quote};

use crate::{
    ast_src::{AstSrc, FieldSrc, KindsSrc, AST_SRC, KINDS_SRC},
    codegen::{self, update, Mode},
    project_root, Result,
};

pub fn generate_syntax(mode: Mode) -> Result<()> {
    let syntax_kinds_file = project_root().join(codegen::SYNTAX_KINDS);
    let syntax_kinds = generate_syntax_kinds(KINDS_SRC)?;
    update(syntax_kinds_file.as_path(), &syntax_kinds, mode)?;

    let ast_nodes_file = project_root().join(codegen::AST_NODES);
    let contents = generate_nodes(KINDS_SRC, AST_SRC)?;
    update(ast_nodes_file.as_path(), &contents, mode)?;

    let ast_tokens_file = project_root().join(codegen::AST_TOKENS);
    let contents = generate_tokens(KINDS_SRC, AST_SRC)?;
    update(ast_tokens_file.as_path(), &contents, mode)?;

    Ok(())
}

#[derive(Debug, Default, Clone)]
struct ElementKinds {
    kinds: BTreeSet<proc_macro2::Ident>,
    has_nodes: bool,
    has_tokens: bool,
}

fn generate_tokens(kinds: KindsSrc<'_>, grammar: AstSrc<'_>) -> Result<String> {
    let all_token_kinds: Vec<_> = kinds
        .punct
        .into_iter()
        .map(|(_, kind)| kind)
        .copied()
        .map(|x| x.into())
        .chain(
            kinds
                .keywords
                .into_iter()
                .chain(kinds.contextual_keywords.into_iter())
                .map(|name| Cow::Owned(format!("{}_KW", to_upper_snake_case(&name)))),
        )
        .chain(kinds.literals.into_iter().copied().map(|x| x.into()))
        .chain(kinds.tokens.into_iter().copied().map(|x| x.into()))
        .collect();

    let tokens = all_token_kinds.iter().map(|kind_str| {
        let kind_str = &**kind_str;
        let kind = format_ident!("{}", kind_str);
        let name = format_ident!("{}", to_pascal_case(kind_str));
        quote! {
            #[derive(Debug, Clone, PartialEq, Eq, Hash)]
            pub struct #name {
                pub(crate) syntax: SyntaxToken,
            }

            impl std::fmt::Display for #name {
                fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                    std::fmt::Display::fmt(&self.syntax, f)
                }
            }

            impl AstToken for #name {
                fn can_cast(kind: SyntaxKind) -> bool { kind == #kind }
                fn cast(syntax: SyntaxToken) -> Option<Self> {
                    if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
                }
                fn syntax(&self) -> &SyntaxToken { &self.syntax }
            }
        }
    });

    let enums = grammar.token_enums.iter().map(|en| {
        let variants = en.variants.iter().map(|var| format_ident!("{}", var)).collect::<Vec<_>>();
        let name = format_ident!("{}", en.name);
        let kinds = variants
            .iter()
            .map(|name| format_ident!("{}", to_upper_snake_case(&name.to_string())))
            .collect::<Vec<_>>();
        assert!(en.traits.is_empty());

        quote! {
                #[derive(Debug, Clone, PartialEq, Eq, Hash)]
                pub enum #name {
                    #(#variants(#variants),)*
                }

                #(
                impl From<#variants> for #name {
                    fn from(node: #variants) -> #name {
                        #name::#variants(node)
                    }
                }
                )*

                impl std::fmt::Display for #name {
                    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                        std::fmt::Display::fmt(self.syntax(), f)
                    }
                }

                impl AstToken for #name {
                    fn can_cast(kind: SyntaxKind) -> bool {
                        match kind {
                            #(#kinds)|* => true,
                            _ => false,
                        }
                    }
                    fn cast(syntax: SyntaxToken) -> Option<Self> {
                        let res = match syntax.kind() {
                            #(
                            #kinds => #name::#variants(#variants { syntax }),
                            )*
                            _ => return None,
                        };
                        Some(res)
                    }
                    fn syntax(&self) -> &SyntaxToken {
                        match self {
                            #(
                            #name::#variants(it) => &it.syntax,
                            )*
                        }
                    }
                }
        }
    });

    crate::reformat(quote! {
        use crate::{SyntaxToken, SyntaxKind::{self, *}, ast::AstToken};

        #(#tokens)*
        #(#enums)*
    })
}

fn generate_nodes(kinds: KindsSrc<'_>, grammar: AstSrc<'_>) -> Result<String> {
    let all_token_kinds: Vec<_> = kinds
        .punct
        .into_iter()
        .map(|(_, kind)| kind)
        .copied()
        .map(|x| x.into())
        .chain(
            kinds
                .keywords
                .into_iter()
                .chain(kinds.contextual_keywords.into_iter())
                .map(|name| Cow::Owned(format!("{}_KW", to_upper_snake_case(&name)))),
        )
        .chain(kinds.literals.into_iter().copied().map(|x| x.into()))
        .chain(kinds.tokens.into_iter().copied().map(|x| x.into()))
        .collect();

    let mut token_kinds = HashSet::new();
    for kind in &all_token_kinds {
        let kind = &**kind;
        let name = to_pascal_case(kind);
        token_kinds.insert(name);
    }

    for en in grammar.token_enums {
        token_kinds.insert(en.name.to_string());
    }

    let nodes = grammar.nodes.iter().map(|node| {
        let name = format_ident!("{}", node.name);
        let kind = format_ident!("{}", to_upper_snake_case(&name.to_string()));
        let traits = node.traits.iter().map(|trait_name| {
            let trait_name = format_ident!("{}", trait_name);
            quote!(impl ast::#trait_name for #name {})
        });

        let methods = node.fields.iter().map(|(name, field)| {
            let method_name = match field {
                FieldSrc::Shorthand => format_ident!("{}", to_lower_snake_case(&name)),
                _ => format_ident!("{}", name),
            };
            let ty = match field {
                FieldSrc::Optional(ty) | FieldSrc::Many(ty) => ty,
                FieldSrc::Shorthand => name,
            };

            let ty = format_ident!("{}", ty);

            match field {
                FieldSrc::Many(_) => {
                    quote! {
                        pub fn #method_name(&self) -> AstChildren<#ty> {
                            support::children(&self.syntax)
                        }
                    }
                }
                FieldSrc::Optional(_) | FieldSrc::Shorthand => {
                    let is_token = token_kinds.contains(&ty.to_string());
                    if is_token {
                        let method_name = format_ident!("{}_token", method_name);
                        quote! {
                            pub fn #method_name(&self) -> Option<#ty> {
                                support::token(&self.syntax)
                            }
                        }
                    } else {
                        quote! {
                            pub fn #method_name(&self) -> Option<#ty> {
                                support::child(&self.syntax)
                            }
                        }
                    }
                }
            }
        });

        quote! {
            #[derive(Debug, Clone, PartialEq, Eq, Hash)]
            pub struct #name {
                pub(crate) syntax: SyntaxNode,
            }

            impl AstNode for #name {
                fn can_cast(kind: SyntaxKind) -> bool {
                    kind == #kind
                }
                fn cast(syntax: SyntaxNode) -> Option<Self> {
                    if Self::can_cast(syntax.kind()) { Some(Self { syntax }) } else { None }
                }
                fn syntax(&self) -> &SyntaxNode { &self.syntax }
            }

            #(#traits)*

            impl #name {
                #(#methods)*
            }
        }
    });

    let enums = grammar.enums.iter().map(|en| {
        let variants = en.variants.iter().map(|var| format_ident!("{}", var)).collect::<Vec<_>>();
        let name = format_ident!("{}", en.name);
        let kinds = variants
            .iter()
            .map(|name| format_ident!("{}", to_upper_snake_case(&name.to_string())))
            .collect::<Vec<_>>();
        let traits = en.traits.iter().map(|trait_name| {
            let trait_name = format_ident!("{}", trait_name);
            quote!(impl ast::#trait_name for #name {})
        });

        quote! {
            #[derive(Debug, Clone, PartialEq, Eq, Hash)]
            pub enum #name {
                #(#variants(#variants),)*
            }

            #(
            impl From<#variants> for #name {
                fn from(node: #variants) -> #name {
                    #name::#variants(node)
                }
            }
            )*

            impl AstNode for #name {
                fn can_cast(kind: SyntaxKind) -> bool {
                    match kind {
                        #(#kinds)|* => true,
                        _ => false,
                    }
                }
                fn cast(syntax: SyntaxNode) -> Option<Self> {
                    let res = match syntax.kind() {
                        #(
                        #kinds => #name::#variants(#variants { syntax }),
                        )*
                        _ => return None,
                    };
                    Some(res)
                }
                fn syntax(&self) -> &SyntaxNode {
                    match self {
                        #(
                        #name::#variants(it) => &it.syntax,
                        )*
                    }
                }
            }

            #(#traits)*
        }
    });

    let displays = grammar
        .enums
        .iter()
        .map(|it| format_ident!("{}", it.name))
        .chain(grammar.nodes.iter().map(|it| format_ident!("{}", it.name)))
        .map(|name| {
            quote! {
                impl std::fmt::Display for #name {
                    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                        std::fmt::Display::fmt(self.syntax(), f)
                    }
                }
            }
        });

    let defined_nodes: HashSet<_> = grammar.nodes.iter().map(|node| node.name).collect();

    for node in kinds
        .nodes
        .iter()
        .map(|kind| to_pascal_case(*kind))
        .filter(|name| !defined_nodes.contains(&**name))
    {
        eprintln!("Warning: node {} not defined in ast source", node);
    }

    let ast = quote! {
        use crate::{
            SyntaxNode, SyntaxKind::{self, *},
            ast::{self, AstNode, AstChildren, support},
        };

        use super::tokens::*;

        #(#nodes)*
        #(#enums)*
        #(#displays)*
    };

    let pretty = crate::reformat(ast)?;
    Ok(pretty)
}

fn generate_syntax_kinds(grammar: KindsSrc<'_>) -> Result<String> {
    let (single_byte_tokens_values, single_byte_tokens): (Vec<_>, Vec<_>) = grammar
        .punct
        .iter()
        .filter(|(token, _name)| token.len() == 1)
        .map(|(token, name)| (token.chars().next().unwrap(), format_ident!("{}", name)))
        .unzip();

    let punctuation_values = grammar.punct.iter().map(|(token, _name)| {
        if "{}[]()".contains(token) {
            let c = token.chars().next().unwrap();
            quote! { #c }
        } else {
            let cs = token.chars().map(|c| Punct::new(c, Spacing::Joint));
            quote! { #(#cs)* }
        }
    });
    let punctuation =
        grammar.punct.iter().map(|(_token, name)| format_ident!("{}", name)).collect::<Vec<_>>();

    let full_keywords_values = &grammar.keywords;
    let full_keywords =
        full_keywords_values.iter().map(|kw| format_ident!("{}_KW", to_upper_snake_case(&kw)));

    let all_keywords_values =
        grammar.keywords.iter().chain(grammar.contextual_keywords.iter()).collect::<Vec<_>>();
    let all_keywords_idents = all_keywords_values.iter().map(|kw| format_ident!("{}", kw));
    let all_keywords = all_keywords_values
        .iter()
        .map(|name| format_ident!("{}_KW", to_upper_snake_case(&name)))
        .collect::<Vec<_>>();

    let literals =
        grammar.literals.iter().map(|name| format_ident!("{}", name)).collect::<Vec<_>>();

    let tokens = grammar.tokens.iter().map(|name| format_ident!("{}", name)).collect::<Vec<_>>();

    let nodes = grammar.nodes.iter().map(|name| format_ident!("{}", name)).collect::<Vec<_>>();

    let ast = quote! {
        #![allow(bad_style, missing_docs, unreachable_pub)]
        /// The kind of syntax node, e.g. `IDENT`, `USE_KW`, or `STRUCT_DEF`.
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
        #[repr(u16)]
        pub enum SyntaxKind {
            // Technical SyntaxKinds: they appear temporally during parsing,
            // but never end up in the final tree
            #[doc(hidden)]
            TOMBSTONE,
            #[doc(hidden)]
            EOF,
            #(#punctuation,)*
            #(#all_keywords,)*
            #(#literals,)*
            #(#tokens,)*
            #(#nodes,)*

            // Technical kind so that we can cast from u16 safely
            #[doc(hidden)]
            __LAST,
        }
        use self::SyntaxKind::*;

        impl SyntaxKind {
            pub fn is_keyword(self) -> bool {
                match self {
                    #(#all_keywords)|* => true,
                    _ => false,
                }
            }

            pub fn is_punct(self) -> bool {
                match self {
                    #(#punctuation)|* => true,
                    _ => false,
                }
            }

            pub fn is_literal(self) -> bool {
                match self {
                    #(#literals)|* => true,
                    _ => false,
                }
            }

            pub fn from_keyword(ident: &str) -> Option<SyntaxKind> {
                let kw = match ident {
                    #(#full_keywords_values => #full_keywords,)*
                    _ => return None,
                };
                Some(kw)
            }

            pub fn from_char(c: char) -> Option<SyntaxKind> {
                let tok = match c {
                    #(#single_byte_tokens_values => #single_byte_tokens,)*
                    _ => return None,
                };
                Some(tok)
            }
        }

        #[macro_export]
        macro_rules! T {
            #((#punctuation_values) => { $crate::SyntaxKind::#punctuation };)*
            #((#all_keywords_idents) => { $crate::SyntaxKind::#all_keywords };)*
        }
    };

    crate::reformat(ast)
}

fn to_upper_snake_case(s: &str) -> String {
    let mut buf = String::with_capacity(s.len());
    let mut prev = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() && prev {
            buf.push('_')
        }
        prev = true;

        buf.push(c.to_ascii_uppercase());
    }
    buf
}

fn to_lower_snake_case(s: &str) -> String {
    let mut buf = String::with_capacity(s.len());
    let mut prev = false;
    for c in s.chars() {
        if c.is_ascii_uppercase() && prev {
            buf.push('_')
        }
        prev = true;

        buf.push(c.to_ascii_lowercase());
    }
    buf
}

fn to_pascal_case(s: &str) -> String {
    let mut buf = String::with_capacity(s.len());
    let mut prev_is_underscore = true;
    for c in s.chars() {
        if c == '_' {
            prev_is_underscore = true;
        } else if prev_is_underscore {
            buf.push(c.to_ascii_uppercase());
            prev_is_underscore = false;
        } else {
            buf.push(c.to_ascii_lowercase());
        }
    }
    buf
}
