use leptos::prelude::*;
// use leptos::{ev::MouseEvent, prelude::*};
// use leptos_node_ref::AnyNodeRef;
// use leptos_struct_component::{struct_component, StructComponent};
// use leptos_style::Style;
use tailwind_fuse::*;

#[derive(TwClass)]
#[tw(
    class = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-all disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg:not([class*='size-'])]:size-4 shrink-0 [&_svg]:shrink-0 outline-none focus-visible:border-ring focus-visible:ring-ring/50 focus-visible:ring-[3px] aria-invalid:ring-destructive/20 dark:aria-invalid:ring-destructive/40 aria-invalid:border-destructive"
)]
pub struct ButtonClass {
    pub variant: ButtonVariant,
    pub size: ButtonSize,
}

#[derive(PartialEq, TwVariant)]
pub enum ButtonVariant {
    #[tw(
        default,
        class = "bg-primary text-primary-foreground shadow-xs hover:bg-primary/90"
    )]
    Default,
    #[tw(
        class = "bg-destructive text-white shadow-xs hover:bg-destructive/90 focus-visible:ring-destructive/20 dark:focus-visible:ring-destructive/40 dark:bg-destructive/60"
    )]
    Destructive,
    #[tw(
        class = "border bg-background shadow-xs hover:bg-accent hover:text-accent-foreground dark:bg-input/30 dark:border-input dark:hover:bg-input/50"
    )]
    Outline,
    #[tw(class = "bg-secondary text-secondary-foreground shadow-xs hover:bg-secondary/80")]
    Secondary,
    #[tw(class = "hover:bg-accent hover:text-accent-foreground dark:hover:bg-accent/50")]
    Ghost,
    #[tw(class = "text-primary underline-offset-4 hover:underline")]
    Link,
}

#[derive(PartialEq, TwVariant)]
pub enum ButtonSize {
    #[tw(default, class = "h-9 px-4 py-2 has-[>svg]:px-3")]
    Default,
    #[tw(class = "h-8 rounded-md gap-1.5 px-3 has-[>svg]:px-2.5")]
    Sm,
    #[tw(class = "h-10 rounded-md px-6 has-[>svg]:px-4")]
    Lg,
    #[tw(class = "size-9")]
    Icon,
}

// #[derive(Clone, StructComponent)]
// #[struct_component(tag = "button")]
// pub struct ButtonChildProps {
//     pub node_ref: AnyNodeRef,

//     // Global attributes
//     pub autofocus: Signal<bool>,
//     pub class: Signal<String>,
//     pub id: MaybeProp<String>,
//     pub style: Signal<Style>,

//     // Attributes from `button`
//     // pub command: MaybeProp<String>,
//     // pub commandfor: MaybeProp<String>,
//     pub disabled: Signal<bool>,
//     pub form: MaybeProp<String>,
//     pub formaction: MaybeProp<String>,
//     pub formenctype: MaybeProp<String>,
//     pub formmethod: MaybeProp<String>,
//     pub formnovalidate: Signal<bool>,
//     pub formtarget: MaybeProp<String>,
//     pub name: MaybeProp<String>,
//     // pub popovertarget: MaybeProp<String>,
//     // pub popovertargetaction: MaybeProp<String>,
//     pub r#type: MaybeProp<String>,
//     pub value: MaybeProp<String>,

//     // Event handler attributes
//     pub onclick: Option<Callback<MouseEvent>>,
// }

// #[component]
// pub fn OldButton(
//     #[prop(into, optional)] variant: Signal<ButtonVariant>,
//     #[prop(into, optional)] size: Signal<ButtonSize>,

//     // Global attributes
//     #[prop(into, optional)] autofocus: Signal<bool>,
//     #[prop(into, optional)] class: MaybeProp<String>,
//     #[prop(into, optional)] id: MaybeProp<String>,
//     #[prop(into, optional)] style: Signal<Style>,

//     // Attributes from `button`
//     // #[prop(into, optional)] command: MaybeProp<String>,
//     // #[prop(into, optional)] commandfor: MaybeProp<String>,
//     #[prop(into, optional)] disabled: Signal<bool>,
//     #[prop(into, optional)] form: MaybeProp<String>,
//     #[prop(into, optional)] formaction: MaybeProp<String>,
//     #[prop(into, optional)] formenctype: MaybeProp<String>,
//     #[prop(into, optional)] formmethod: MaybeProp<String>,
//     #[prop(into, optional)] formnovalidate: Signal<bool>,
//     #[prop(into, optional)] formtarget: MaybeProp<String>,
//     #[prop(into, optional)] name: MaybeProp<String>,
//     // #[prop(into, optional)] popovertarget: MaybeProp<String>,
//     // #[prop(into, optional)] popovertargetaction: MaybeProp<String>,
//     #[prop(into, optional)] r#type: MaybeProp<String>,
//     #[prop(into, optional)] value: MaybeProp<String>,

//     // Event handler attributes
//     #[prop(into, optional)] onclick: Option<Callback<MouseEvent>>,

//     #[prop(into, optional)] node_ref: AnyNodeRef,
//     #[prop(into, optional)] as_child: Option<Callback<ButtonChildProps, AnyView>>,
//     #[prop(optional)] children: Option<Children>,
// ) -> impl IntoView {
//     let class = Memo::new(move |_| {
//         ButtonClass {
//             variant: variant.get(),
//             size: size.get(),
//         }
//         .with_class(class.get().unwrap_or_default())
//     });

//     let child_props = ButtonChildProps {
//         node_ref,

//         // Global attributes
//         autofocus,
//         class: class.into(),
//         id,
//         style,

//         // Attributes from `button`
//         // command,
//         // commandfor,
//         disabled,
//         form,
//         formaction,
//         formenctype,
//         formmethod,
//         formnovalidate,
//         formtarget,
//         name,
//         // popovertarget,
//         // popovertargetaction,
//         r#type,
//         value,

//         // Event handler attributes
//         onclick,
//     };

//     if let Some(as_child) = as_child.as_ref() {
//         as_child.run(child_props)
//     } else {
//         child_props.render(children)
//     }
// }

#[component]
pub fn Button(
    #[prop(into, optional)] variant: Signal<ButtonVariant>,
    #[prop(into, optional)] size: Signal<ButtonSize>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] disabled: MaybeProp<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = Memo::new(move |_| {
        tw_merge!(
            ButtonClass {
                variant: variant.get(),
                size: size.get(),
            }
            .to_class(),
            class.get()
        )
    });

    view! {
        <button
            class={move || class.get()}
            disabled=move || disabled.get().unwrap_or(false)
        >{ children.map(|c| c().into_any()).unwrap_or_else(|| ().into_any()) }</button>
    }
}
