use leptos::prelude::*;
use tailwind_fuse::*;

#[component]
pub fn Table(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <div
            data-slot="table-container"
            class="relative w-full overflow-x-auto"
        >
            <table
                data-slot="table"
                class=move || tw_merge!(
                    "w-full caption-bottom text-sm",
                    class.get()
                )
            >
                { children }
            </table>
        </div>
    }
}

#[component]
pub fn TableHeader(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <thead
            data-slot="table-header"
            class=move || tw_merge!(
                "[&_tr]:border-b",
                class.get()
            )
        >
            { children }
        </thead>
    }
}

#[component]
pub fn TableBody(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <tbody
            data-slot="table-body"
            class=move || tw_merge!(
                "[&_tr:last-child]:border-0",
                class.get()
            )
        >
            { children }
        </tbody>
    }
}

#[component]
pub fn TableFooter(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <tfoot
            data-slot="table-footer"
            class=move || tw_merge!(
                "bg-muted/50 border-t font-medium [&>tr]:last:border-b-0",
                class.get()
            )
        >
            { children }
        </tfoot>
    }
}

#[component]
pub fn TableRow(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <tr
            data-slot="table-row"
            class=move || tw_merge!(
                "hover:bg-muted/50 data-[state=selected]:bg-muted border-b transition-colors",
                class.get()
            )
        >
            { children }
        </tr>
    }
}

#[component]
pub fn TableHead(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <th
            data-slot="table-head"
            class=move || tw_merge!(
                "text-foreground h-10 px-2 text-left align-middle font-medium whitespace-nowrap [&:has([role=checkbox])]:pr-0 [&>[role=checkbox]]:translate-y-[2px]",
                class.get()
            )
        >
            { children }
        </th>
    }
}

#[component]
pub fn TableCell(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <td
            data-slot="table-cell"
            class=move || tw_merge!(
                "p-2 align-middle whitespace-nowrap [&:has([role=checkbox])]:pr-0 [&>[role=checkbox]]:translate-y-[2px]",
                class.get()
            )
        >
            { children }
        </td>
    }
}

#[component]
pub fn TableCaption(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let children = children
        .map(|c| c().into_any())
        .unwrap_or_else(|| ().into_any());
    view! {
        <td
            data-slot="table-caption"
            class=move || tw_merge!(
                "text-muted-foreground mt-4 text-sm",
                class.get()
            )
        >
            { children }
        </td>
    }
}
