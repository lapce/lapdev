extern crate proc_macro;

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, ToTokens};
use syn::{
    braced,
    ext::IdentExt,
    parenthesized,
    parse::{Parse, ParseStream},
    parse_macro_input, parse_quote,
    spanned::Spanned,
    token::Comma,
    AttrStyle, Attribute, FnArg, Ident, Pat, PatType, ReturnType, Token, Type, Visibility,
};

/// Accumulates multiple errors into a result.
/// Only use this for recoverable errors, i.e. non-parse errors. Fatal errors should early exit to
/// avoid further complications.
macro_rules! extend_errors {
    ($errors: ident, $e: expr) => {
        match $errors {
            Ok(_) => $errors = Err($e),
            Err(ref mut errors) => errors.extend($e),
        }
    };
}

struct RpcMethod {
    attrs: Vec<Attribute>,
    ident: Ident,
    args: Vec<FnArg>,
    output: ReturnType,
}

impl Parse for RpcMethod {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let attrs = input.call(Attribute::parse_outer)?;
        input.parse::<Token![async]>()?;
        input.parse::<Token![fn]>()?;
        let ident = input.parse()?;
        let content;
        parenthesized!(content in input);
        let mut args = Vec::new();
        let mut errors = Ok(());
        for arg in content.parse_terminated(FnArg::parse, Comma)? {
            match &arg {
                FnArg::Typed(captured) if matches!(&*captured.pat, Pat::Ident(_)) => {
                    args.push(arg);
                }
                FnArg::Typed(captured) => {
                    extend_errors!(
                        errors,
                        syn::Error::new(captured.pat.span(), "patterns aren't allowed in RPC args")
                    );
                }
                FnArg::Receiver(_) => {
                    // args.push(arg);
                }
            }
        }
        errors?;
        let output = input.parse()?;
        input.parse::<Token![;]>()?;

        Ok(Self {
            attrs,
            ident,
            args,
            output,
        })
    }
}

struct Service {
    vis: Visibility,
    ident: Ident,
    attrs: Vec<Attribute>,
    rpcs: Vec<RpcMethod>,
}

impl Parse for Service {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let vis = input.parse()?;
        input.parse::<Token![trait]>()?;
        let ident: Ident = input.parse()?;
        let attrs = input.call(Attribute::parse_outer)?;

        let content;
        braced!(content in input);
        let mut rpcs = Vec::<RpcMethod>::new();
        while !content.is_empty() {
            rpcs.push(content.parse()?);
        }

        let mut ident_errors = Ok(());
        for rpc in &rpcs {
            if rpc.ident == "new" {
                extend_errors!(
                    ident_errors,
                    syn::Error::new(
                        rpc.ident.span(),
                        format!(
                            "method name conflicts with generated fn `{}Client::new`",
                            ident.unraw()
                        )
                    )
                );
            }
        }
        ident_errors?;

        Ok(Self {
            vis,
            ident,
            attrs,
            rpcs,
        })
    }
}

struct ServiceGenerator<'a> {
    vis: &'a Visibility,
    service_ident: &'a Ident,
    rpcs: &'a [RpcMethod],
    return_types: &'a [&'a Type],
    attrs: &'a [Attribute],
    client_ident: &'a Ident,
    method_idents: &'a [&'a Ident],
    arg_pats: &'a [Vec<&'a Pat>],
    request_ident: &'a Ident,
    response_ident: &'a Ident,
    // derives: Option<&'a TokenStream2>,
    camel_case_idents: &'a [Ident],
    args: &'a [Vec<&'a PatType>],
    // request_names: &'a [String],
    method_cfgs: &'a [Vec<&'a Attribute>],
}

impl ToTokens for ServiceGenerator<'_> {
    fn to_tokens(&self, output: &mut TokenStream2) {
        output.extend(vec![
            self.trait_service(),
            self.enum_request(),
            self.enum_response(),
            self.struct_client(),
            self.impl_client_new(),
        ]);
    }
}

impl ServiceGenerator<'_> {
    fn trait_service(&self) -> TokenStream2 {
        let &Self {
            vis,
            attrs,
            service_ident,
            request_ident,
            response_ident,
            rpcs,
            return_types,
            method_idents,
            camel_case_idents,
            arg_pats,
            ..
        } = self;

        let rpc_fns = rpcs.iter().zip(return_types.iter()).map(
            |(
                RpcMethod {
                    attrs, ident, args, ..
                },
                output,
            )| {
                quote! {
                    #( #attrs )*
                    async fn #ident(&self, headers: &::lapdev_hrpc::HeaderMap, #( #args ),*) -> #output;
                }
            },
        );

        quote! {
            #( #attrs )*
            #vis trait #service_ident: ::core::marker::Sized {
                #( #rpc_fns )*

                async fn handle_rpc(&self, headers: &::lapdev_hrpc::HeaderMap, body: &str) -> ::lapdev_hrpc::anyhow::Result<#response_ident> {
                    let req: Result<#request_ident, ::lapdev_hrpc::serde_json::Error> = ::lapdev_hrpc::serde_json::from_str(body);
                    let req = req?;

                    match req {
                        #(
                            #request_ident::#camel_case_idents{ #( #arg_pats ),* } => {
                                Ok(#response_ident::#camel_case_idents(self.#method_idents(headers, #( #arg_pats ),*).await))
                            }
                        )*
                    }
                }
            }
        }
    }

    fn struct_client(&self) -> TokenStream2 {
        let &Self {
            vis, client_ident, ..
        } = self;

        quote! {
            #[allow(unused)]
            #[derive(Clone, Debug)]
            /// The client stub that makes RPC calls to the server. All request methods return
            /// [Futures](::core::future::Future).
            #vis struct #client_ident(String);
        }
    }

    fn impl_client_new(&self) -> TokenStream2 {
        let &Self {
            client_ident,
            method_idents,
            vis,
            request_ident,
            arg_pats,
            return_types,
            args,
            camel_case_idents,
            response_ident,
            ..
        } = self;

        quote! {
            impl #client_ident
            {
                pub fn new(url: String) -> Self {
                    Self(url)
                }

                #(
                    #[allow(unused)]
                    #vis async fn #method_idents(&self, #( #args ),*) -> ::lapdev_hrpc::anyhow::Result<#return_types> {
                        let request = #request_ident::#camel_case_idents { #( #arg_pats ),* };
                        // let resp = self.0.call(ctx, request);
                        let resp: #response_ident = ::lapdev_hrpc::gloo_net::http::Request::post(&self.0).json(&request)?.send().await?.json().await?;
                        match resp {
                            #response_ident::#camel_case_idents(msg) => Ok(msg),
                            _ => ::core::unreachable!(),
                        }
                    }
                )*
            }
        }
    }

    fn enum_request(&self) -> TokenStream2 {
        let &Self {
            // derives,
            vis,
            request_ident,
            camel_case_idents,
            args,
            method_cfgs,
            ..
        } = self;

        quote! {
            /// The request sent over the wire from the client to the server.
            #[allow(missing_docs)]
            #[derive(Debug, ::lapdev_hrpc::serde::Serialize, ::lapdev_hrpc::serde::Deserialize)]
            #vis enum #request_ident {
                #(
                    #( #method_cfgs )*
                    #camel_case_idents{ #( #args ),* }
                ),*
            }
            // impl ::tarpc::RequestName for #request_ident {
            //     fn name(&self) -> &str {
            //         match self {
            //             #(
            //                 #( #method_cfgs )*
            //                 #request_ident::#camel_case_idents{..} => {
            //                     #request_names
            //                 }
            //             )*
            //         }
            //     }
            // }
        }
    }

    fn enum_response(&self) -> TokenStream2 {
        let &Self {
            vis,
            response_ident,
            camel_case_idents,
            return_types,
            ..
        } = self;

        quote! {
            /// The response sent over the wire from the server to the client.
            #[allow(missing_docs)]
            #[derive(Debug, ::lapdev_hrpc::serde::Serialize, ::lapdev_hrpc::serde::Deserialize)]
            #vis enum #response_ident {
                #( #camel_case_idents(#return_types) ),*
            }
        }
    }
}

#[proc_macro_attribute]
pub fn service(_attr: TokenStream, input: TokenStream) -> TokenStream {
    let unit_type: &Type = &parse_quote!(());
    let Service {
        ref vis,
        ref ident,
        ref attrs,
        ref rpcs,
    } = parse_macro_input!(input as Service);

    let camel_case_fn_names: &Vec<_> = &rpcs
        .iter()
        .map(|rpc| snake_to_camel(&rpc.ident.unraw().to_string()))
        .collect();

    let methods = rpcs.iter().map(|rpc| &rpc.ident).collect::<Vec<_>>();
    let args: &[Vec<&PatType>] = &rpcs
        .iter()
        .map(|rpc| {
            rpc.args
                .iter()
                .filter_map(|a| {
                    if let FnArg::Typed(p) = a {
                        Some(p)
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();

    // let request_names = methods
    //     .iter()
    //     .map(|m| format!("{ident}.{m}"))
    //     .collect::<Vec<_>>();

    ServiceGenerator {
        vis,
        service_ident: ident,
        rpcs,
        return_types: &rpcs
            .iter()
            .map(|rpc| match rpc.output {
                ReturnType::Type(_, ref ty) => ty.as_ref(),
                ReturnType::Default => unit_type,
            })
            .collect::<Vec<_>>(),
        attrs,
        request_ident: &format_ident!("{}Request", ident),
        response_ident: &format_ident!("{}Response", ident),
        client_ident: &format_ident!("{}Client", ident),
        method_idents: &methods,
        arg_pats: &args
            .iter()
            .map(|args| args.iter().map(|arg| &*arg.pat).collect())
            .collect::<Vec<_>>(),
        camel_case_idents: &rpcs
            .iter()
            .zip(camel_case_fn_names.iter())
            .map(|(rpc, name)| Ident::new(name, rpc.ident.span()))
            .collect::<Vec<_>>(),
        args,
        method_cfgs: &collect_cfg_attrs(rpcs),
        // request_names: &request_names,
    }
    .into_token_stream()
    .into()
}

fn snake_to_camel(ident_str: &str) -> String {
    let mut camel_ty = String::with_capacity(ident_str.len());

    let mut last_char_was_underscore = true;
    for c in ident_str.chars() {
        match c {
            '_' => last_char_was_underscore = true,
            c if last_char_was_underscore => {
                camel_ty.extend(c.to_uppercase());
                last_char_was_underscore = false;
            }
            c => camel_ty.extend(c.to_lowercase()),
        }
    }

    camel_ty.shrink_to_fit();
    camel_ty
}

fn collect_cfg_attrs(rpcs: &[RpcMethod]) -> Vec<Vec<&Attribute>> {
    rpcs.iter()
        .map(|rpc| {
            rpc.attrs
                .iter()
                .filter(|att| {
                    att.style == AttrStyle::Outer
                        && match &att.meta {
                            syn::Meta::List(syn::MetaList { path, .. }) => {
                                path.get_ident() == Some(&Ident::new("cfg", rpc.ident.span()))
                            }
                            _ => false,
                        }
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>()
}
