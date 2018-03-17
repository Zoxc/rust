// Copyright 2018 The Rust Project Developers. See the COPYRIGHT
// file at the top-level directory of this distribution and at
// http://rust-lang.org/COPYRIGHT.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use rustc::session::Session;
use syntax::ast::*;
use syntax::ptr::P;
use syntax::fold;
use syntax::fold::Folder;
use syntax::symbol::keywords;
use syntax::codemap::dummy_spanned;
use syntax_pos::DUMMY_SP;
use syntax_pos::symbol::Symbol;

struct ModFolder<'a> {
    use_std: &'a Item,
    root: Option<Ident>,
    main: Symbol,
}

impl<'a> ModFolder<'a> {
    fn fold_qpath(&mut self, qself: &mut Option<QSelf>, path: &mut Path) {
        let old = path.segments.len();
        *path = self.fold_path(path.clone());
        let add = path.segments.len() - old;
        qself.as_mut().map(|qself| {
            qself.position += add;
            qself.ty = self.fold_ty(qself.ty.clone());
        });
    }
}

impl<'a> fold::Folder for ModFolder<'a> {
    fn fold_use_tree(&mut self, mut use_tree: UseTree) -> UseTree {
        if let Some(root) = self.root {
            let pos = {
                let get = |i| {
                    use_tree.prefix.segments.get(i).map(|p: &PathSegment| p.identifier.name)
                };
                if get(0) == Some(keywords::SelfValue.name()) ||
                   get(0) == Some(keywords::Super.name()) {
                    None
                } else {
                    let mut i = 0;
                    if get(i) == Some(keywords::CrateRoot.name()) {
                        i += 1;
                    }
                    if get(i) == Some(keywords::Crate.name()) {
                        i += 1;
                    }
                    Some(i)
                }
            };
            if let Some(pos) = pos {
                use_tree.prefix.segments.insert(pos, PathSegment {
                    identifier: root,
                    span: use_tree.span,
                    parameters: None,
                });
            }
            use_tree
        } else {
            fold::noop_fold_use_tree(use_tree, self)
        }
    }

    fn fold_path(&mut self, mut p: Path) -> Path {
        if let Some(root) = self.root {
            if let Some(first) = p.segments.first().cloned() {
                if first.identifier.name == keywords::CrateRoot.name() {
                    let idx = if p.segments.get(1).map(|p| p.identifier.name) ==
                                    Some(keywords::Crate.name()) {
                        2
                    } else {
                        1
                    };
                    p.segments.insert(idx, PathSegment {
                        identifier: root,
                        span: p.span,
                        parameters: None,
                    });
                }
            }
            fold::noop_fold_path(p, self)
        } else {
            fold::noop_fold_path(p, self)
        }
    }

    fn fold_ty(&mut self, mut t: P<Ty>) -> P<Ty> {
        if match t.node  {
            TyKind::Path(ref mut qself, ref mut path) => {
                self.fold_qpath(qself, path);
                true
            }
            _ => false,
        } {
            return t;
        }
        fold::noop_fold_ty(t, self)
    }

    fn fold_pat(&mut self, mut p: P<Pat>) -> P<Pat> {
        if match p.node  {
            PatKind::Path(ref mut qself, ref mut path) => {
                self.fold_qpath(qself, path);
                true
            }
            _ => false,
        } {
            return p;
        }
        fold::noop_fold_pat(p, self)
    }

    fn fold_expr(&mut self, mut e: P<Expr>) -> P<Expr> {
        if match e.node  {
            ExprKind::Path(ref mut qself, ref mut path) => {
                self.fold_qpath(qself, path);
                true
            }
            _ => false,
        } {
            return e;
        }
        e.map(|e| fold::noop_fold_expr(e, self))
    }

    fn fold_item_simple(&mut self, mut item: Item) -> Item {
        if self.root.is_some() {
            fold::noop_fold_item_simple(item, self)
        } else {
            let is_root = match item.node {
                ItemKind::Mod(ref mut module) => {
                    for mut item in &mut module.items {
                        if item.ident.name == self.main {
                            item.vis = dummy_spanned(VisibilityKind::Public);
                        }
                    }
                    true
                }
                _ => false,
            };

            if is_root {
                let mut folder = ModFolder {
                    use_std: self.use_std,
                    root: Some(item.ident),
                    main: self.main,
                };

                let mut item = fold::noop_fold_item_simple(item, &mut folder);
                match item.node {
                    ItemKind::Mod(ref mut module) => {
                        module.items.push(P(self.use_std.clone()));
                    }
                    _ => (),
                }
                item
            } else {
                fold::noop_fold_item_simple(item, self)
            }
        }
    }

    fn fold_mac(&mut self, _mac: Mac) -> Mac {
        fold::noop_fold_mac(_mac, self)
    }
}

pub fn modify_crate(_session: &Session, krate: Crate) -> Crate {
    let std_i = Ident::from_str("std");
    let use_std = Item {
        ident: keywords::Invalid.ident(),
        attrs: Vec::new(),
        id: DUMMY_NODE_ID,
        node: ItemKind::Use(P(UseTree {
            span: DUMMY_SP,
            kind: UseTreeKind::Simple(Some(std_i)),
            prefix: Path {
                segments: vec![PathSegment {
                    identifier: std_i,
                    span: DUMMY_SP,
                    parameters: None,
                }],
                span: DUMMY_SP,
            },
        })),
        vis: dummy_spanned(VisibilityKind::Inherited),
        span: DUMMY_SP,
        tokens: None,
    };
    let Crate {
        module,
        attrs,
        span,
    } = krate;
    let module = ModFolder {
        use_std: &use_std,
        root: None,
        main: Symbol::intern("main"),
    }.fold_mod(module);
    Crate {
        module,
        attrs,
        span,
    }
}
