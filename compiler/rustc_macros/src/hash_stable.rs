use proc_macro2::{Ident, Span};
use quote::quote;
use syn::{LitStr, parse_quote};

struct Attributes {
    ignore: bool,
    project: Option<Ident>,
}

fn parse_attributes(field: &syn::Field) -> Attributes {
    let mut attrs = Attributes { ignore: false, project: None };
    for attr in &field.attrs {
        let meta = &attr.meta;
        if !meta.path().is_ident("stable_hasher") {
            continue;
        }
        let mut any_attr = false;
        let _ = attr.parse_nested_meta(|nested| {
            if nested.path.is_ident("ignore") {
                attrs.ignore = true;
                any_attr = true;
            }
            if nested.path.is_ident("project") {
                let _ = nested.parse_nested_meta(|meta| {
                    if attrs.project.is_none() {
                        attrs.project = meta.path.get_ident().cloned();
                    }
                    any_attr = true;
                    Ok(())
                });
            }
            Ok(())
        });
        if !any_attr {
            panic!("error parsing stable_hasher");
        }
    }
    attrs
}

pub(crate) fn hash_stable_derive(s: synstructure::Structure<'_>) -> proc_macro2::TokenStream {
    hash_stable_derive_with_mode(s, HashStableMode::Normal)
}

pub(crate) fn hash_stable_generic_derive(
    s: synstructure::Structure<'_>,
) -> proc_macro2::TokenStream {
    hash_stable_derive_with_mode(s, HashStableMode::Generic)
}

pub(crate) fn hash_stable_no_context_derive(
    s: synstructure::Structure<'_>,
) -> proc_macro2::TokenStream {
    hash_stable_derive_with_mode(s, HashStableMode::NoContext)
}

enum HashStableMode {
    // Use the query-system aware stable hashing context.
    Normal,
    // Emit a generic implementation that uses a crate-local `StableHashingContext`
    // trait, when the crate is upstream of `rustc_middle`.
    Generic,
    // Emit a hash-stable implementation that takes no context,
    // and emits per-field where clauses for (almost-)perfect derives.
    NoContext,
}

fn hash_stable_derive_with_mode(
    mut s: synstructure::Structure<'_>,
    mode: HashStableMode,
) -> proc_macro2::TokenStream {
    let generic: syn::GenericParam = match mode {
        HashStableMode::Normal => parse_quote!('__ctx),
        HashStableMode::Generic | HashStableMode::NoContext => parse_quote!(__CTX),
    };

    // no_context impl is able to derive by-field, which is closer to a perfect derive.
    s.add_bounds(match mode {
        HashStableMode::Normal | HashStableMode::Generic => synstructure::AddBounds::Generics,
        HashStableMode::NoContext => synstructure::AddBounds::Fields,
    });

    // For generic impl, add `where __CTX: HashStableContext`.
    match mode {
        HashStableMode::Normal => {}
        HashStableMode::Generic => {
            s.add_where_predicate(parse_quote! { __CTX: crate::HashStableContext });
        }
        HashStableMode::NoContext => {}
    }

    s.add_impl_generic(generic);

    let discriminant = hash_stable_discriminant(&mut s);
    let body = hash_stable_body(&mut s);
    let structure = hash_stable_structure_body(&mut s);

    let context: syn::Type = match mode {
        HashStableMode::Normal => {
            parse_quote!(::rustc_query_system::ich::StableHashingContext<'__ctx>)
        }
        HashStableMode::Generic | HashStableMode::NoContext => parse_quote!(__CTX),
    };

    s.bound_impl(
        quote!(
            ::rustc_data_structures::stable_hasher::HashStable<
                #context
            >
        ),
        quote! {
            #[inline]
            fn hash_stable(
                &self,
                __hcx: &mut #context,
                __hasher: &mut ::rustc_data_structures::stable_hasher::StableHasher) {
                #discriminant
                match *self { #body }
            }
                #[inline]
                fn structure<'s, W: ::rustc_data_structures::inspect::Write>(&self, __state: &mut ::rustc_data_structures::stable_hasher::StructureState<'s, #context, W>) -> ::rustc_data_structures::inspect::Value {
                match *self { #structure }
            }
        },
    )
}

fn hash_stable_structure_body(s: &mut synstructure::Structure<'_>) -> proc_macro2::TokenStream {
    let is_enum = matches!(s.ast().data, syn::Data::Enum(_));
    let type_ident = &s.ast().ident;

    // Build match arms for each variant where we intern a schema and then
    // return a `Value::Schema` with positional field values.
    s.each_variant(|vi| {
        let var_ident = &vi.ast().ident;

        match &vi.ast().fields {
            syn::Fields::Named(named) => {
                let mut field_names: Vec<LitStr> = Vec::new();
                let mut pushes = proc_macro2::TokenStream::new();
                for binding in vi.bindings().iter() {
                    let attrs = parse_attributes(binding.ast());
                    if attrs.ignore {
                        continue;
                    }

                    let name = binding
                        .ast()
                        .ident
                        .as_ref()
                        .expect("named fields must have idents")
                        .to_string();
                    field_names.push(LitStr::new(&name, Span::call_site()));

                    let bind_ident = &binding.binding;
                    let push = if let Some(project) = attrs.project {
                        quote! { values.push((#bind_ident.#project).structure(__state)); }
                    } else {
                        quote! { values.push(#bind_ident.structure(__state)); }
                    };
                    pushes.extend(push);
                }

                let field_count = field_names.len();

                if is_enum {
                    quote! {
                        {
                            static FIELDS: [&'static str; #field_count] = [#(#field_names),*];
                            static SCHEMA: ::rustc_data_structures::inspect::SchemaRef =
                                ::rustc_data_structures::inspect::SchemaRef::new(
                                    ::rustc_data_structures::inspect::Schema::Enum {
                                        path: concat!(module_path!(), "::", stringify!(#type_ident)),
                                        variant_name: stringify!(#var_ident),
                                        variant: ::rustc_data_structures::inspect::EnumVariantSchema::Named(&FIELDS),
                                    },
                                );
                            let __id = __state.intern_schema(&SCHEMA);
                            let mut values: Vec<::rustc_data_structures::inspect::Value> = Vec::new();
                            #pushes
                            ::rustc_data_structures::inspect::Value::Schema { id: __id, values }
                        }
                    }
                } else {
                    let _ = named;
                    quote! {
                        {
                            static FIELDS: [&'static str; #field_count] = [#(#field_names),*];
                            static SCHEMA: ::rustc_data_structures::inspect::SchemaRef =
                                ::rustc_data_structures::inspect::SchemaRef::new(
                                    ::rustc_data_structures::inspect::Schema::Struct {
                                        path: concat!(module_path!(), "::", stringify!(#type_ident)),
                                        fields: &FIELDS,
                                    },
                                );
                            let __id = __state.intern_schema(&SCHEMA);
                            let mut values: Vec<::rustc_data_structures::inspect::Value> = Vec::new();
                            #pushes
                            ::rustc_data_structures::inspect::Value::Schema { id: __id, values }
                        }
                    }
                }
            }
            syn::Fields::Unnamed(_) => {
                let mut count: u32 = 0;
                let mut pushes = proc_macro2::TokenStream::new();
                for binding in vi.bindings().iter() {
                    let attrs = parse_attributes(binding.ast());
                    if attrs.ignore {
                        continue;
                    }
                    count += 1;
                    let bind_ident = &binding.binding;
                    let push = if let Some(project) = attrs.project {
                        quote! { values.push((#bind_ident.#project).structure(__state)); }
                    } else {
                        quote! { values.push(#bind_ident.structure(__state)); }
                    };
                    pushes.extend(push);
                }

                if is_enum {
                    quote! {
                        {
                            static SCHEMA: ::rustc_data_structures::inspect::SchemaRef =
                                ::rustc_data_structures::inspect::SchemaRef::new(
                                    ::rustc_data_structures::inspect::Schema::Enum {
                                        path: concat!(module_path!(), "::", stringify!(#type_ident)),
                                        variant_name: stringify!(#var_ident),
                                        variant: ::rustc_data_structures::inspect::EnumVariantSchema::Tuple(#count),
                                    },
                                );
                            let __id = __state.intern_schema(&SCHEMA);
                            let mut values: Vec<::rustc_data_structures::inspect::Value> = Vec::new();
                            #pushes
                            ::rustc_data_structures::inspect::Value::Schema { id: __id, values }
                        }
                    }
                } else {
                    quote! {
                        {
                            static SCHEMA: ::rustc_data_structures::inspect::SchemaRef =
                                ::rustc_data_structures::inspect::SchemaRef::new(
                                    ::rustc_data_structures::inspect::Schema::StructTuple {
                                        path: concat!(module_path!(), "::", stringify!(#type_ident)),
                                        field_count: #count,
                                    },
                                );
                            let __id = __state.intern_schema(&SCHEMA);
                            let mut values: Vec<::rustc_data_structures::inspect::Value> = Vec::new();
                            #pushes
                            ::rustc_data_structures::inspect::Value::Schema { id: __id, values }
                        }
                    }
                }
            }
            syn::Fields::Unit => {
                if is_enum {
                    quote! {
                        {
                            static SCHEMA: ::rustc_data_structures::inspect::SchemaRef =
                                ::rustc_data_structures::inspect::SchemaRef::new(
                                    ::rustc_data_structures::inspect::Schema::Enum {
                                        path: concat!(module_path!(), "::", stringify!(#type_ident)),
                                        variant_name: stringify!(#var_ident),
                                        variant: ::rustc_data_structures::inspect::EnumVariantSchema::Unit,
                                    },
                                );
                            let __id = __state.intern_schema(&SCHEMA);
                            ::rustc_data_structures::inspect::Value::Schema { id: __id, values: Vec::new() }
                        }
                    }
                } else {
                    quote! {
                        {
                            static SCHEMA: ::rustc_data_structures::inspect::SchemaRef =
                                ::rustc_data_structures::inspect::SchemaRef::new(
                                    ::rustc_data_structures::inspect::Schema::Struct {
                                        path: concat!(module_path!(), "::", stringify!(#type_ident)),
                                        fields: &[],
                                    },
                                );
                            let __id = __state.intern_schema(&SCHEMA);
                            ::rustc_data_structures::inspect::Value::Schema { id: __id, values: Vec::new() }
                        }
                    }
                }
            }
        }
    })
}

fn hash_stable_discriminant(s: &mut synstructure::Structure<'_>) -> proc_macro2::TokenStream {
    match s.ast().data {
        syn::Data::Enum(_) => quote! {
            ::std::mem::discriminant(self).hash_stable(__hcx, __hasher);
        },
        syn::Data::Struct(_) => quote! {},
        syn::Data::Union(_) => panic!("cannot derive on union"),
    }
}

fn hash_stable_body(s: &mut synstructure::Structure<'_>) -> proc_macro2::TokenStream {
    s.each(|bi| {
        let attrs = parse_attributes(bi.ast());
        if attrs.ignore {
            quote! {}
        } else if let Some(project) = attrs.project {
            quote! {
                (&#bi.#project).hash_stable(__hcx, __hasher);
            }
        } else {
            quote! {
                #bi.hash_stable(__hcx, __hasher);
            }
        }
    })
}
