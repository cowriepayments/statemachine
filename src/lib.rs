use std::collections::{HashMap, HashSet};
use proc_macro::TokenStream;
use syn::{ parse_macro_input, braced, token, Ident, Result, Token };
use syn::parse::{ Parse, ParseStream };
use syn::punctuated::Punctuated;
use quote::{quote, format_ident};
use convert_case::{Case, Casing};

struct Machine {
    name: Ident,
    shared_data_type: Option<Ident>,
    #[allow(dead_code)]
    brace_token: token::Brace,
    states: Punctuated<StateDefinition, Token![,]>
}

struct StateDefinition {
    init: bool,
    name: Ident,
    associated_data_type: Option<Ident>,
    #[allow(dead_code)]
    brace_token: token::Brace,
    transitions: Punctuated<StateTransition, Token![,]>
}

struct StateTransition {
    event: Ident,
    #[allow(dead_code)]
    separator: Token![=>],
    next_state: Ident
}

impl Parse for Machine {
    fn parse(input: ParseStream) -> Result<Self> {
        let name: Ident = input.parse()?;
        
        let mut shared_data_type: Option<Ident> = None;
        let colon: Result<Token![:]> = input.parse();
        if colon.is_ok() {
            shared_data_type = Some(input.parse()?);
        }

        let content;
        Ok(Machine {
            name,
            shared_data_type,
            brace_token: braced!(content in input),
            states: content.parse_terminated(StateDefinition::parse)?,
        })
    }
}

impl Parse for StateDefinition {
    fn parse(input: ParseStream) -> Result<Self> {
        let mut init = false;
        let name: Ident;

        let x: Ident = input.parse()?;
        if x == "init" {
            init = true;
            name = input.parse()?;
        } else {
            name = x;
        }

        let mut associated_data_type: Option<Ident> = None;
        let colon: Result<Token![:]> = input.parse();
        if colon.is_ok() {
            associated_data_type = Some(input.parse()?);
        }

        let content;
        Ok(StateDefinition {
            init,
            name,
            associated_data_type,
            brace_token: braced!(content in input),
            transitions: content.parse_terminated(StateTransition::parse)?,
        })
    }
}

impl Parse for StateTransition {
    fn parse(input: ParseStream) -> Result<Self> {
        Ok(StateTransition {
            event: input.parse()?,
            separator: input.parse()?,
            next_state: input.parse()?
        })
    }
}

#[proc_macro]
pub fn statemachine(input: TokenStream) -> TokenStream {
    let m = parse_macro_input!(input as Machine);

    let mut init_states = HashSet::new();
    let mut state_data_types = HashMap::new();
    for state in m.states.iter() {
        if state.init {
            init_states.insert(&state.name);
        };
        if let Some(dt) = &state.associated_data_type {
            state_data_types.insert(&state.name, dt);
        }
    }

    let state_names: Vec<&Ident> = m.states.iter().map(|x| &x.name).collect();

    let parent_name = &m.name;
    let wrapped_type = format_ident!("{}{}", "Wrapped", parent_name);
    let shared_data_type = &m.shared_data_type;
    
    let state_structs = m.states.iter().map(|x| {
        let state_name = &x.name;
        let data_type = &x.associated_data_type;

        match data_type {
            Some(dt) => quote! {
                pub struct #state_name {
                    data: #dt
                }

                impl #state_name {
                    fn new(data: #dt) -> Self {
                        Self {
                            data
                        }
                    }

                    pub fn data(&self) -> &#dt {
                        &self.data
                    }
                }
            },
            None => quote! {
                pub struct #state_name {}

                impl #state_name {
                    fn new() -> Self {
                        Self {}
                    }
                }
            }
        }
    });

    let transitions_block = m.states.iter().fold(quote!(), |acc, x| {
        let state_name = &x.name;
        let exit_fn_name = format_ident!("{}_{}", "on_exit", state_name.to_string().to_case(Case::Snake));

        let transitions = x.transitions.iter().map(|y| {
            let event = &y.event;
            let next_state_name = &y.next_state;
            let arg = state_data_types.get(next_state_name);
            let enter_fn_name = format_ident!("{}_{}", "on_enter", next_state_name.to_string().to_case(Case::Snake));

            let exit_call = match shared_data_type {
                Some(_) => match state_data_types.get(state_name) {
                    Some(_) => quote! {
                        self.observer.#exit_fn_name(ctx, &self.id, State::#next_state_name, &self.data, &self.state.data).await.map_err(|e| TransitionError::ObserverError(e))?;
                    },
                    None => quote! {
                        self.observer.#exit_fn_name(ctx, &self.id, State::#next_state_name, &self.data).await.map_err(|e| TransitionError::ObserverError(e))?;
                    }
                },
                None => match state_data_types.get(state_name) {
                    Some(_) => quote! {
                        self.observer.#exit_fn_name(ctx, &self.id, State::#next_state_name, &self.state.data).await.map_err(|e| TransitionError::ObserverError(e))?;
                    },
                    None => quote! {
                        self.observer.#exit_fn_name(ctx, &self.id, State::#next_state_name).await.map_err(|e| TransitionError::ObserverError(e))?;
                    }
                }
            };

            let enter_from_type = if init_states.contains(next_state_name) {
                quote!(Some(State::#state_name))
            } else {
                quote!(State::#state_name)
            };

            match arg {
                Some(a) => match shared_data_type {
                    Some(_) => quote! {
                        impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                            pub async fn #event(mut self, ctx: &mut S, data: #a) -> Result<#parent_name<#next_state_name, S, T>, TransitionError<T::Error>> {
                                self.observer.on_transition(ctx, &self.id, State::#state_name, State::#next_state_name, Some(&self.data), Some(&data)).await.map_err(|e| TransitionError::ObserverError(e))?;
                                #exit_call
                                self.observer.#enter_fn_name(ctx, &self.id, #enter_from_type, &self.data, &data).await.map_err(|e| TransitionError::ObserverError(e))?;
                                Ok(#parent_name::<#next_state_name, S, T>::new(self.observer, self.id, #next_state_name::new(data), self.data))
                            }
                        }
                    },
                    None => quote! {
                        impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                            pub async fn #event(mut self, ctx: &mut S, data: #a) -> Result<#parent_name<#next_state_name, S, T>, TransitionError<T::Error>> {
                                self.observer.on_transition(ctx, &self.id, State::#state_name, State::#next_state_name, Option::<()>::None, Some(&data)).await.map_err(|e| TransitionError::ObserverError(e))?;
                                #exit_call
                                self.observer.#enter_fn_name(ctx, &self.id, #enter_from_type, &data).await.map_err(|e| TransitionError::ObserverError(e))?;
                                Ok(#parent_name::<#next_state_name, S, T>::new(self.observer, self.id, #next_state_name::new(data)))
                            }
                        }
                    }
                },
                None => match shared_data_type {
                    Some(_) => quote! {
                        impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                            pub async fn #event(mut self, ctx: &mut S) -> Result<#parent_name<#next_state_name, S, T>, TransitionError<T::Error>> {
                                self.observer.on_transition(ctx, &self.id, State::#state_name, State::#next_state_name, Some(&self.data), Option::<()>::None).await.map_err(|e| TransitionError::ObserverError(e))?;
                                #exit_call
                                self.observer.#enter_fn_name(ctx, &self.id, #enter_from_type, &self.data).await.map_err(|e| TransitionError::ObserverError(e))?;
                                Ok(#parent_name::<#next_state_name, S, T>::new(self.observer, self.id, #next_state_name::new(), self.data))
                            }
                        }
                    },
                    None => quote! {
                        impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                            pub async fn #event(mut self, ctx: &mut S) -> Result<#parent_name<#next_state_name, S, T>, TransitionError<T::Error>> {
                                self.observer.on_transition(ctx, &self.id, State::#state_name, State::#next_state_name, Option::<()>::None, Option::<()>::None).await.map_err(|e| TransitionError::ObserverError(e))?;
                                #exit_call
                                self.observer.#enter_fn_name(ctx, &self.id, #enter_from_type).await.map_err(|e| TransitionError::ObserverError(e))?;
                                Ok(#parent_name::<#next_state_name, S, T>::new(self.observer, self.id, #next_state_name::new()))
                            }
                        }
                    }
                }
            }
        });

        quote! {
            #acc

            #(#transitions)*
        }
    });

    let parent_state_impls = m.states.iter().map(|x| {
        let state_name = &x.name;
        let enter_fn_name = format_ident!("{}_{}", "on_enter", state_name.to_string().to_case(Case::Snake));

        let common_methods = quote! {
            pub fn id(&self) -> &str {
                &self.id
            }
        };

        match shared_data_type {
            Some(sdt) => {
                let constructor = quote! {
                    impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                        fn new(observer: T, id: String, state: #state_name, data: #sdt) -> Self {
                            Self {
                                observer,
                                id,
                                state,
                                data,
                                phantom: PhantomData
                            }
                        }

                        pub fn data(&self) -> &#sdt {
                            &self.data
                        }

                        #common_methods
                    }
                };

                match x.init {
                    false => constructor,
                    true => match &x.associated_data_type {
                        Some(dt) => quote! {
                            #constructor
    
                            impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                                pub async fn init(ctx: &mut S, mut observer: T, id: Option<String>, data: #sdt, state_data: #dt) -> Result<Self, InitError<T::Error>> {
                                    let id = observer.on_init(ctx, id, State::#state_name, Some(&data), Some(&state_data)).await.map_err(|e| InitError::ObserverError(e))?.ok_or(InitError::EmptyId)?;
                                    observer.#enter_fn_name(ctx, &id, None, &data, &state_data).await.map_err(|e| InitError::ObserverError(e))?;
                                    Ok(Self::new(observer, id, #state_name::new(state_data), data))
                                }
                            }
                        },
                        None => quote! {
                            #constructor
    
                            impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                                pub async fn init(ctx: &mut S, mut observer: T, id: Option<String>, data: #sdt) -> Result<Self, InitError<T::Error>> {
                                    let id = observer.on_init(ctx, id, State::#state_name, Some(&data), Option::<()>::None).await.map_err(|e| InitError::ObserverError(e))?.ok_or(InitError::EmptyId)?;
                                    observer.#enter_fn_name(ctx, &id, None, &data).await.map_err(|e| InitError::ObserverError(e))?;
                                    Ok(Self::new(observer, id, #state_name::new(), data))
                                }
                            }
                        }
                    }
                }
            },
            None => {
                let constructor = quote! {
                    impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                        fn new(observer: T, id: String, state: #state_name) -> Self {
                            Self {
                                observer,
                                id,
                                state,
                                phantom: PhantomData
                            }
                        }

                        #common_methods
                    }
                };

                match x.init {
                    false => constructor,
                    true => match &x.associated_data_type {
                        Some(dt) => quote! {
                            #constructor
    
                            impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                                pub async fn init(ctx: &mut S, mut observer: T, id: Option<String>, state_data: #dt) -> Result<Self, InitError<T::Error>> {
                                    let id = observer.on_init(ctx, id, State::#state_name, Option::<()>::None, Some(&state_data)).await.map_err(|e| InitError::ObserverError(e))?.ok_or(InitError::EmptyId)?;
                                    observer.#enter_fn_name(ctx, &id, None, &state_data).await.map_err(|e| InitError::ObserverError(e))?;
                                    Ok(Self::new(observer, id, #state_name::new(state_data)))
                                }
                            }
                        },
                        None => quote! {
                            #constructor
    
                            impl<S: Send, T: Observer<S> + Send> #parent_name<#state_name, S, T> {
                                pub async fn init(ctx: &mut S, mut observer: T, id: Option<String>) -> Result<Self, InitError<T::Error>> {
                                    let id = observer.on_init(ctx, id, State::#state_name, Option::<()>::None, Option::<()>::None).await.map_err(|e| InitError::ObserverError(e))?.ok_or(InitError::EmptyId)?;
                                    observer.#enter_fn_name(ctx, &id, None).await.map_err(|e| InitError::ObserverError(e))?;
                                    Ok(Self::new(observer, id, #state_name::new()))
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    let parent_struct = match shared_data_type {
        Some(sdt) => quote! {
            pub struct #parent_name<S, T: Send, U: Observer<T> + Send> {
                observer: U,
                id: String,
                pub state: S,
                data: #sdt,
                phantom: PhantomData<T>
            }
        },
        None => quote! {
            pub struct #parent_name<S, T: Send, U: Observer<T> + Send> {
                observer: U,
                id: String,
                pub state: S,
                phantom: PhantomData<T>
            }
        }
    };

    let restore_fns = m.states.iter().map(|x| {
        let state_name = &x.name;
        let expected_state_dt = state_data_types.get(state_name);

        let fn_name = format_ident!("{}_{}", "restore", state_name.to_string().to_case(Case::Snake));

        match shared_data_type {
            Some(shared_dt) => {
                match expected_state_dt {
                    Some(state_dt) => quote! {
                        async fn #fn_name<S: Send, T: Observer<S> + Send>(mut observer: T, id: String, shared_d_enc: Option<Encoded>, state_d_enc: Option<Encoded>) -> Result<#wrapped_type<S, T>, RestoreError> {
                            let shared_d_enc_some = shared_d_enc.ok_or(RestoreError::EmptyData)?;
                            let shared_d: #shared_dt = match shared_d_enc_some {
                                Encoded::Json(data) => serde_json::from_value(data).ok().ok_or(RestoreError::InvalidData)?
                            };

                            let state_d_enc_some = state_d_enc.ok_or(RestoreError::EmptyData)?;
                            let state_d: #state_dt = match state_d_enc_some {
                                Encoded::Json(data) => serde_json::from_value(data).ok().ok_or(RestoreError::InvalidData)?
                            };

                            Ok(#wrapped_type::#state_name(#parent_name::<#state_name, S, T>::new(observer, id, #state_name::new(state_d), shared_d)))
                        }
                    },
                    None => quote! {
                        async fn #fn_name<S: Send, T: Observer<S> + Send>(mut observer: T, id: String, shared_d_enc: Option<Encoded>, state_d_enc: Option<Encoded>) -> Result<#wrapped_type<S, T>, RestoreError> {
                            let shared_d_enc_some = shared_d_enc.ok_or(RestoreError::EmptyData)?;
                            let shared_d: #shared_dt = match shared_d_enc_some {
                                Encoded::Json(data) => serde_json::from_value(data).ok().ok_or(RestoreError::InvalidData)?
                            };

                            if state_d_enc.is_some() {
                                return Err(RestoreError::UnexpectedData)
                            };

                            Ok(#wrapped_type::#state_name(#parent_name::<#state_name, S, T>::new(observer, id, #state_name::new(), shared_d)))
                        }
                    }
                }
            },
            None => {
                match expected_state_dt {
                    Some(state_dt) => quote! {
                        async fn #fn_name<S: Send, T: Observer + Send>(mut observer: T, id: String, shared_d_enc: Option<Encoded>, state_d_enc: Option<Encoded>) -> Result<#wrapped_type<S, T>, RestoreError> {
                            if shared_d_enc.is_some() {
                                return Err(RestoreError::UnexpectedData)
                            };

                            let state_d_enc_some = state_d_enc.ok_or(RestoreError::EmptyData)?;
                            let state_d: #state_dt = match state_d_enc_some {
                                Encoded::Json(data) => serde_json::from_value(data).ok().ok_or(RestoreError::InvalidData)?
                            };

                            Ok(#wrapped_type::#state_name(#parent_name::<#state_name, S, T>::new(observer, id, #state_name::new(state_d))))
                        }
                    },
                    None => quote! {
                        async fn #fn_name<S: Send, T: Observer + Send>(mut observer: T, id: String, shared_d_enc: Option<Encoded>, state_d_enc: Option<Encoded>) -> Result<#wrapped_type<S, T>, RestoreError> {
                            if shared_d_enc.is_some() {
                                return Err(RestoreError::UnexpectedData)
                            };

                            if state_d_enc.is_some() {
                                return Err(RestoreError::UnexpectedData)
                            };

                            Ok(#wrapped_type::#state_name(#parent_name::<#state_name, S, T>::new(observer, id, #state_name::new())))
                        }
                    }
                }
            }
        }
    });

    let restore_arms = m.states.iter().map(|x| {
        let state_name = &x.name;
        let fn_name = format_ident!("{}_{}", "restore", state_name.to_string().to_case(Case::Snake));
        quote!(stringify!(#state_name) => #fn_name(observer, id, data, state_data).await)
    });

    let listeners = m.states.iter().map(|x| {
        let state_name = &x.name;
        let enter_fn_name = format_ident!("{}_{}", "on_enter", state_name.to_string().to_case(Case::Snake));
        let exit_fn_name = format_ident!("{}_{}", "on_exit", state_name.to_string().to_case(Case::Snake));

        let from_type = if x.init {
            quote!(Option<State>)
        } else {
            quote!(State)
        };

        let maybe_data_type = state_data_types.get(state_name);
        match shared_data_type {
            Some(sdt) => match maybe_data_type {
                Some(data_type) => quote! {
                    async fn #enter_fn_name(&mut self, ctx: &mut S, id: &str, from: #from_type, data: &#sdt, state_data: &#data_type) -> Result<(), Self::Error> {
                        Ok(())
                    }
                    async fn #exit_fn_name(&mut self, ctx: &mut S, id: &str, to: State, data: &#sdt, state_data: &#data_type) -> Result<(), Self::Error> {
                        Ok(())
                    }
                },
                None => quote! {
                    async fn #enter_fn_name(&mut self, ctx: &mut S, id: &str, from: #from_type, data: &#sdt) -> Result<(), Self::Error> {
                        Ok(())
                    }
                    async fn #exit_fn_name(&mut self, ctx: &mut S, id: &str, to: State, data: &#sdt) -> Result<(), Self::Error> {
                        Ok(())
                    }
                }
            },
            None => match maybe_data_type {
                Some(data_type) => quote! {
                    async fn #enter_fn_name(&mut self, ctx: &mut S, id: &str, from: #from_type, state_data: &#data_type) -> Result<(), Self::Error> {
                        Ok(())
                    }
                    async fn #exit_fn_name(&mut self, ctx: &mut S, id: &str, to: State, state_data: &#data_type) -> Result<(), Self::Error> {
                        Ok(())
                    }
                },
                None => quote! {
                    async fn #enter_fn_name(&mut self, ctx: &mut S, id: &str, from: #from_type) -> Result<(), Self::Error> {
                        Ok(())
                    }
                    async fn #exit_fn_name(&mut self, ctx: &mut S, id: &str, to: State) -> Result<(), Self::Error> {
                        Ok(())
                    }
                }
            }
        }

    });

    let out = quote! {
        #[derive(Debug)]
        pub enum InitError<T> {
            EmptyId,
            ObserverError(T)
        }

        #[derive(Debug)]
        pub enum TransitionError<T> {
            ObserverError(T)
        }

        #[derive(Debug)]
        pub enum RestoreError {
            EmptyData,
            UnexpectedData,
            InvalidData,
            InvalidState
        }

        #[derive(Debug)]
        pub enum RetrieveError<T> {
            RestoreError(RestoreError),
            RetrieverError(T)
        }
        
        pub enum State {
            #(#state_names),*
        }

        impl State {
            pub fn to_string(&self) -> String {
                match self {
                    #(State::#state_names => String::from(stringify!(#state_names))),*
                }
            }
        }
        
        #[async_trait]
        pub trait Observer<S: Send> {
            type Error;

            async fn on_init<T: Serialize + Send, U: Serialize + Send>(&mut self, ctx: &mut S, id: Option<String>, to: State, data: Option<T>, state_data: Option<U>) -> Result<Option<String>, Self::Error> {
                Ok(id)
            }
            
            async fn on_transition<T: Serialize + Send, U: Serialize + Send>(&mut self, ctx: &mut S, id: &str, from: State, to: State, data: Option<T>, state_data: Option<U>) -> Result<(), Self::Error> {
                Ok(())
            }

            #(#listeners)*
        }

        #[async_trait]
        pub trait Retriever<T: Send> {
            type RetrieverError;

            async fn on_retrieve(&mut self, ctx: &mut T, id: &str) -> Result<(String, Option<Encoded>, Option<Encoded>), Self::RetrieverError>;
        }

        #parent_struct
        #(#state_structs)*
        #(#parent_state_impls)*
        #transitions_block

        pub enum #wrapped_type<S: Send, T: Observer<S> + Send> {
            #(#state_names(#parent_name<#state_names, S, T>)),*
        }

        pub enum Encoded {
            Json(serde_json::Value)
        }

        #(#restore_fns)*

        pub async fn restore<S: Send, T: Observer<S> + Send>(mut observer: T, id: String, state_string: String, data: Option<Encoded>, state_data: Option<Encoded>) -> Result<#wrapped_type<S, T>, RestoreError> {
            let state_str: &str = &state_string;
            match state_str {
                #(#restore_arms,)*
                _ => Err(RestoreError::InvalidState)
            }
        }

        pub async fn retrieve<S: Send, T: Retriever<S> + Observer<S> + Send>(ctx: &mut S, mut retriever: T, id: String) -> Result<#wrapped_type<S, T>, RetrieveError<T::RetrieverError>> {
            let id_str: &str = &id;
            let (state_string, maybe_data, maybe_state_data) = retriever.on_retrieve(ctx, id_str).await.map_err(|e| RetrieveError::RetrieverError(e))?;
            restore(retriever, id, state_string, maybe_data, maybe_state_data).await.map_err(|e| RetrieveError::RestoreError(e))
        }
    };

    out.into()
}