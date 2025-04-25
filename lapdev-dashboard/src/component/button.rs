use leptos::prelude::*;
// use leptos::{ev::MouseEvent, prelude::*};
// use leptos_node_ref::AnyNodeRef;
// use leptos_struct_component::{struct_component, StructComponent};
// use leptos_style::Style;
use tailwind_fuse::*;
use web_sys::MouseEvent;

#[derive(TwClass)]
#[tw(
    class = "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium ring-offset-background transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:size-4 [&_svg]:shrink-0"
)]
pub struct ButtonClass {
    pub variant: ButtonVariant,
    pub size: ButtonSize,
}

#[derive(PartialEq, TwVariant)]
pub enum ButtonVariant {
    #[tw(
        default,
        class = "bg-primary text-primary-foreground hover:bg-primary/90"
    )]
    Default,
    #[tw(class = "bg-destructive text-destructive-foreground hover:bg-destructive/90")]
    Destructive,
    #[tw(class = "border border-input bg-background hover:bg-accent hover:text-accent-foreground")]
    Outline,
    #[tw(class = "bg-secondary text-secondary-foreground hover:bg-secondary/80")]
    Secondary,
    #[tw(class = "hover:bg-accent hover:text-accent-foreground")]
    Ghost,
    #[tw(class = "text-primary underline-offset-4 hover:underline")]
    Link,
}

#[derive(PartialEq, TwVariant)]
pub enum ButtonSize {
    #[tw(default, class = "h-10 px-4 py-2")]
    Default,
    #[tw(class = "h-9 rounded-md px-3")]
    Sm,
    #[tw(class = "h-11 rounded-md px-8")]
    Lg,
    #[tw(class = "h-10 w-10")]
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
    #[prop(into, optional)] onclick: Option<Callback<MouseEvent>>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = Memo::new(move |_| {
        ButtonClass {
            variant: variant.get(),
            size: size.get(),
        }
        .with_class(class.get().unwrap_or_default())
    });

    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <button
            class={move || class.get()}
            on:click=move |e| onclick.unwrap().run(e)
        >{ children }</button>
    }
}
