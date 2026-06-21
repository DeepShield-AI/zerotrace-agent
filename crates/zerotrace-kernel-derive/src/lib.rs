//! Derive macros for the `ZeroTrace` DI framework.
//!
//! - [`Bundle`] — auto-implements `zerotrace_kernel::bundle::Bundle`
//! - [`SignalType`] — auto-implements `zerotrace_core::signal::SignalType`
//!
//! # Usage
//!
//! ```ignore
//! use zerotrace_kernel_derive::{Bundle, SignalType};
//!
//! #[derive(Bundle)]
//! #[bundle(id = "my_bundle", name = "My Bundle")]
//! struct MyBundle { }
//!
//! #[derive(SignalType)]
//! #[signal(kind = "ai.anomaly")]
//! struct AiAnomaly { score: f64 }
//! ```
//!
//! With pipeline and components:
//!
//! ```ignore
//! # use std::sync::Arc;
//! # use parking_lot::RwLock;
//! # use zerotrace_kernel_derive::Bundle;
//! # struct DbPool; struct AppService; struct AuthService; struct OptionalFeature;
//! #[derive(Bundle)]
//! #[bundle(id = "my_bundle", name = "My Bundle")]
//! #[bundle(pipeline(
//!     name = "main",
//!     sources = ["cpu", "mem"],
//!     processors = ["tag"],
//!     reporters = ["http"]
//! ))]
//! struct MyBundle {
//!     #[component(id = "db", deps = [])]
//!     db: Arc<RwLock<DbPool>>,
//!     #[component(id = "svc", deps = [DbPool])]
//!     svc: Arc<RwLock<AppService>>,
//!     #[component(id = "opt", deps = [AuthService], optional)]
//!     optional_comp: Arc<RwLock<OptionalFeature>>,
//! }
//! ```

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, LitStr, parse_macro_input};

/// Parsed bundle-level attributes.
struct BundleAttrs {
    id: String,
    name: String,
    required: bool,
    pipelines: Vec<PipelineTemplate>,
}

struct PipelineTemplate {
    name: String,
    sources: Vec<String>,
    processors: Vec<String>,
    reporters: Vec<String>,
}

impl BundleAttrs {
    fn from_attrs(attrs: &[syn::Attribute]) -> Result<Self, syn::Error> {
        let mut id: Option<String> = None;
        let mut name: Option<String> = None;
        let mut required = false;
        let mut pipelines = Vec::new();

        for attr in attrs {
            if !attr.path().is_ident("bundle") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("id") {
                    let val: LitStr = meta.value()?.parse()?;
                    id = Some(val.value());
                    Ok(())
                } else if meta.path.is_ident("name") {
                    let val: LitStr = meta.value()?.parse()?;
                    name = Some(val.value());
                    Ok(())
                } else if meta.path.is_ident("required") {
                    required = true;
                    Ok(())
                } else if meta.path.is_ident("pipeline") {
                    // Parse pipeline(...) with named arguments
                    let mut pipe_name = None;
                    let mut sources = Vec::new();
                    let mut processors = Vec::new();
                    let mut reporters = Vec::new();

                    meta.parse_nested_meta(|pipe_meta| {
                        if pipe_meta.path.is_ident("name") {
                            let val: LitStr = pipe_meta.value()?.parse()?;
                            pipe_name = Some(val.value());
                            Ok(())
                        } else if pipe_meta.path.is_ident("sources") {
                            sources = parse_string_list(&pipe_meta)?;
                            Ok(())
                        } else if pipe_meta.path.is_ident("processors") {
                            processors = parse_string_list(&pipe_meta)?;
                            Ok(())
                        } else if pipe_meta.path.is_ident("reporters") {
                            reporters = parse_string_list(&pipe_meta)?;
                            Ok(())
                        } else {
                            Err(pipe_meta
                                .error("expected `name`, `sources`, `processors`, or `reporters`"))
                        }
                    })?;

                    pipelines.push(PipelineTemplate {
                        name: pipe_name.unwrap_or_else(|| "default".into()),
                        sources,
                        processors,
                        reporters,
                    });
                    Ok(())
                } else {
                    Err(meta.error("expected `id`, `name`, `required`, or `pipeline`"))
                }
            })?;
        }

        let bid = id.ok_or_else(|| {
            syn::Error::new(
                proc_macro2::Span::call_site(),
                "missing #[bundle(id = \"...\")]",
            )
        })?;
        let bname = name.unwrap_or_else(|| bid.clone());
        Ok(BundleAttrs {
            id: bid,
            name: bname,
            required,
            pipelines,
        })
    }
}

/// Extract the inner type `T` from a bundle field type `Arc<RwLock<T>>`.
///
/// Returns `None` if the type doesn't match the expected pattern, in which
/// case the caller should fall back to using the raw field type for
/// `provides` (preserving backward compatibility with non-standard field
/// types).
fn unwrap_arc_rwlock_inner(ty: &syn::Type) -> Option<proc_macro2::TokenStream> {
    // We expect: Arc < RwLock < T > >
    // Parse outer path: Arc
    let outer_path = match ty {
        syn::Type::Path(type_path) => type_path,
        _ => return None,
    };
    let outer_seg = outer_path.path.segments.last()?;
    if outer_seg.ident != "Arc" {
        return None;
    }
    // Get Arc's generic arg: RwLock<T>
    let arc_args = match &outer_seg.arguments {
        syn::PathArguments::AngleBracketed(args) => &args.args,
        _ => return None,
    };
    let rwlock_ty: &syn::Type = match arc_args.first()? {
        syn::GenericArgument::Type(ty) => ty,
        _ => return None,
    };
    // Parse inner path: RwLock
    let inner_path = match rwlock_ty {
        syn::Type::Path(type_path) => type_path,
        _ => return None,
    };
    let inner_seg = inner_path.path.segments.last()?;
    if inner_seg.ident != "RwLock" {
        return None;
    }
    // Get RwLock's generic arg: T
    let rwlock_args = match &inner_seg.arguments {
        syn::PathArguments::AngleBracketed(args) => &args.args,
        _ => return None,
    };
    let inner_ty: &syn::Type = match rwlock_args.first()? {
        syn::GenericArgument::Type(ty) => ty,
        _ => return None,
    };
    Some(quote!(#inner_ty))
}

/// Parse a bracketed string list like `["a", "b"]` from a meta value.
fn parse_string_list(meta: &syn::meta::ParseNestedMeta<'_>) -> syn::Result<Vec<String>> {
    let value_stream = meta.value()?;
    let content;
    syn::bracketed!(content in value_stream);
    let mut vals = Vec::new();
    while !content.is_empty() {
        let s: LitStr = content.parse()?;
        vals.push(s.value());
        if content.peek(syn::Token![,]) {
            let _: syn::Token![,] = content.parse()?;
        }
    }
    Ok(vals)
}

/// Parsed per-field component attributes.
struct ComponentAttrs {
    id: String,
    /// Dependency types stored as token streams (preserving span info,
    /// avoiding a String round-trip).  Each token stream represents a
    /// type that will be used as `TypeId::of::<Type>()`.
    deps: Vec<proc_macro2::TokenStream>,
    optional: bool,
}

impl ComponentAttrs {
    fn from_attrs(attrs: &[syn::Attribute]) -> Result<Self, syn::Error> {
        let mut id = None;
        let mut deps = Vec::new();
        let mut optional = false;
        for attr in attrs {
            if !attr.path().is_ident("component") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("id") {
                    let val: LitStr = meta.value()?.parse()?;
                    id = Some(val.value());
                    Ok(())
                } else if meta.path.is_ident("optional") {
                    optional = true;
                    Ok(())
                } else if meta.path.is_ident("deps") {
                    let value_stream = meta.value()?;
                    let content;
                    syn::bracketed!(content in value_stream);
                    let mut vals = Vec::new();
                    while !content.is_empty() {
                        let ty: syn::Type = content.parse()?;
                        vals.push(quote!(#ty));
                        if content.peek(syn::Token![,]) {
                            let _: syn::Token![,] = content.parse()?;
                        }
                    }
                    deps = vals;
                    Ok(())
                } else {
                    Err(meta.error("expected `id`, `deps`, or `optional`"))
                }
            })?;
        }
        Ok(ComponentAttrs {
            id: id.unwrap_or_else(|| "unnamed".to_string()),
            deps,
            optional,
        })
    }
}

/// Derive `Bundle` for a struct whose fields are `Arc<RwLock<T>>`.
///
/// Each field annotated with `#[component(...)]` becomes a
/// `ComponentDescriptor` in the generated `components()` method.
#[proc_macro_derive(Bundle, attributes(bundle, component))]
pub fn derive_bundle(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_bundle(input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.into_compile_error().into(),
    }
}

fn expand_bundle(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = input.ident.clone();
    let bundle_attrs = BundleAttrs::from_attrs(&input.attrs)
        .map_err(|e| syn::Error::new_spanned(&struct_name, e))?;

    let bundle_id = &bundle_attrs.id;
    let bundle_name = &bundle_attrs.name;
    let bundle_required = bundle_attrs.required;

    // Build default_pipelines if specified
    let pipeline_builders: Vec<proc_macro2::TokenStream> = bundle_attrs
        .pipelines
        .iter()
        .map(|p| {
            let pname = &p.name;
            let srcs = &p.sources;
            let procs = &p.processors;
            let reps = &p.reporters;
            quote! {
                zerotrace_kernel::bundle::PipelineTemplate {
                    name: #pname.into(),
                    sources: vec![#(#srcs.into()),*],
                    processors: vec![#(#procs.into()),*],
                    reporters: vec![#(#reps.into()),*],
                }
            }
        })
        .collect();

    // Extract fields
    let fields = match input.data {
        syn::Data::Struct(ref data) => match data.fields {
            syn::Fields::Named(ref fields) => &fields.named,
            _ => {
                return Err(syn::Error::new_spanned(
                    &input,
                    "Bundle can only be derived for structs with named fields",
                ));
            },
        },
        _ => {
            return Err(syn::Error::new_spanned(
                &input,
                "Bundle can only be derived for structs",
            ));
        },
    };

    // Build component descriptors from annotated fields
    let mut component_builders = Vec::new();

    for field in fields.iter() {
        let comp_attrs = match ComponentAttrs::from_attrs(&field.attrs) {
            Ok(a) => a,
            Err(_) => continue,
        };

        let field_name = field.ident.as_ref().expect("named field has no ident");

        let comp_id = &comp_attrs.id;
        let optional = comp_attrs.optional;

        let dep_type_ids: Vec<proc_macro2::TokenStream> = comp_attrs
            .deps
            .iter()
            .map(|dep_tokens| {
                quote! { std::any::TypeId::of::<#dep_tokens>() }
            })
            .collect();

        let field_ty = &field.ty;

        // Extract the inner type T from Arc<RwLock<T>> for the provides key.
        // The convention is that Bundle fields are always Arc<RwLock<Inner>>,
        // and `provides` should be TypeId::of::<Inner> to match how
        // downstream bundles declare deps and how World::get::<Inner>()
        // looks up resources.
        let provides_ty = unwrap_arc_rwlock_inner(field_ty).unwrap_or_else(|| quote!(#field_ty));

        component_builders.push(quote! {
            {
                let field_arc: #field_ty = self.#field_name.clone();
                zerotrace_kernel::bundle::ComponentDescriptor {
                    id: #comp_id,
                    provides: std::any::TypeId::of::<#provides_ty>(),
                    deps: vec![#(#dep_type_ids),*],
                    optional: #optional,
                    factory: Box::new(move |_world: &zerotrace_kernel::world::World,
                                            _lifecycle: &zerotrace_kernel::lifecycle::LifecycleRegistry| {
                        Ok(field_arc.clone() as std::sync::Arc<dyn std::any::Any + Send + Sync>)
                    }),
                }
            }
        });
    }

    let default_pipelines_block = if pipeline_builders.is_empty() {
        quote! {}
    } else {
        quote! {
            fn default_pipelines(&self) -> Vec<zerotrace_kernel::bundle::PipelineTemplate> {
                vec![#(#pipeline_builders),*]
            }
        }
    };

    let required_block = if bundle_required {
        quote! {
            fn required(&self) -> bool { true }
        }
    } else {
        quote! {}
    };

    let expanded = quote! {
        impl zerotrace_kernel::bundle::Bundle for #struct_name {
            fn id(&self) -> &'static str {
                #bundle_id
            }
            fn name(&self) -> &'static str {
                #bundle_name
            }
            fn components(&self) -> Vec<zerotrace_kernel::bundle::ComponentDescriptor> {
                vec![
                    #(#component_builders),*
                ]
            }
            #default_pipelines_block
            #required_block
        }
    };

    Ok(expanded)
}

// ── SignalType derive ──────────────────────────────────────────────

/// Parsed signal-level attributes.
struct SignalAttrs {
    kind: String,
}

impl SignalAttrs {
    fn from_attrs(attrs: &[syn::Attribute]) -> Result<Self, String> {
        let mut kind = None;
        for attr in attrs {
            if !attr.path().is_ident("signal") {
                continue;
            }
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("kind") {
                    let val: syn::LitStr = meta.value()?.parse()?;
                    kind = Some(val.value());
                    Ok(())
                } else {
                    Err(meta.error("expected `kind`"))
                }
            })
            .map_err(|e| e.to_string())?;
        }
        Ok(SignalAttrs {
            kind: kind.unwrap_or_else(|| "custom".to_string()),
        })
    }
}

/// Derive [`SignalType`](zerotrace_core::signal::SignalType) for a struct.
///
/// The `#[signal(kind = "...")]` attribute sets the [`SignalKind`] label.
/// Known signal kinds (`metric`, `trace`, `log`, `profile`, `event`) map to
/// their respective [`SignalKind`] constants; everything else is passed
/// directly to the [`SignalKind`] struct constructor.
///
/// ```ignore
/// // (proc-macro crate — needs zerotrace-core in scope;
/// //  tested via zerotrace-kernel integration tests instead)
/// use zerotrace_core::signal::{SignalType, SignalKind};
/// use zerotrace_kernel_derive::SignalType;
///
/// #[derive(Debug, Clone, SignalType)]
/// #[signal(kind = "ai.anomaly")]
/// struct AiAnomaly {
///     pub score: f64,
/// }
///
/// assert_eq!(AiAnomaly::signal_kind(), SignalKind("ai.anomaly"));
/// ```
#[proc_macro_derive(SignalType, attributes(signal))]
pub fn derive_signal_type(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand_signal_type(input) {
        Ok(tokens) => tokens.into(),
        Err(e) => {
            let msg = e.to_string();
            quote!(compile_error!(#msg)).into()
        },
    }
}

fn expand_signal_type(input: DeriveInput) -> syn::Result<proc_macro2::TokenStream> {
    let struct_name = &input.ident;
    let signal_attrs = SignalAttrs::from_attrs(&input.attrs)
        .map_err(|e| syn::Error::new_spanned(struct_name, e))?;

    let kind_str = &signal_attrs.kind;
    // Map well-known strings to the built-in constants; everything else
    // uses the struct constructor directly (SignalKind is a newtype struct,
    // not an enum, so custom kinds are just SignalKind("my.kind")).
    let kind_tokens: proc_macro2::TokenStream = match kind_str.as_str() {
        "metric" => quote!(zerotrace_core::signal::SignalKind::METRIC),
        "trace" => quote!(zerotrace_core::signal::SignalKind::TRACE),
        "log" => quote!(zerotrace_core::signal::SignalKind::LOG),
        "profile" => quote!(zerotrace_core::signal::SignalKind::PROFILE),
        "event" => quote!(zerotrace_core::signal::SignalKind::EVENT),
        custom => {
            quote!(zerotrace_core::signal::SignalKind(#custom))
        },
    };

    let expanded = quote! {
        impl zerotrace_core::signal::SignalType for #struct_name {
            fn signal_kind() -> zerotrace_core::signal::SignalKind {
                #kind_tokens
            }
        }
    };

    Ok(expanded)
}
