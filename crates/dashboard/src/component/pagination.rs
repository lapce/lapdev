use lapdev_common::kube::PaginatedInfo;
use leptos::prelude::*;
use tailwind_fuse::*;

use crate::component::{
    button::{ButtonClass, ButtonVariant},
    select::{Select, SelectContent, SelectItem, SelectTrigger},
};

use super::button::ButtonSize;

// Pagination component variants
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PaginationSize {
    Default,
    Sm,
    Lg,
}

impl PaginationSize {
    fn to_class(&self) -> &'static str {
        match self {
            PaginationSize::Default => "h-9 px-3",
            PaginationSize::Sm => "h-8 px-2 text-xs",
            PaginationSize::Lg => "h-11 px-4",
        }
    }
}

// Base Pagination container
#[component]
pub fn Pagination(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <nav
            role="navigation"
            aria-label="pagination"
            data-slot="pagination"
            class=move || class.get()
        >

            {children.map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
        </nav>
    }
}

// Pagination content wrapper - contains all pagination items
#[component]
pub fn PaginationContent(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <ul
            data-slot="pagination-content"
            class=move || {
                tw_merge!("flex flex-row items-center gap-1",
                    class.get())
            }
        >
            {children.map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
        </ul>
    }
}

// Individual pagination item wrapper
#[component]
pub fn PaginationItem(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    view! {
        <li data-slot="pagination-item" class=move || class.get()>
            {children.map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
        </li>
    }
}

// Base pagination link/button component
#[component]
pub fn PaginationButton(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] size: MaybeProp<ButtonSize>,
    #[prop(into, optional)] is_active: MaybeProp<bool>,
    #[prop(into, optional)] disabled: MaybeProp<bool>,
    #[prop(optional)] children: Option<Children>,
) -> impl IntoView {
    let class = Memo::new(move |_| {
        tw_merge!(
            ButtonClass {
                variant: if is_active.get().unwrap_or(false) {
                    ButtonVariant::Outline
                } else {
                    ButtonVariant::Ghost
                },
                size: size.get().unwrap_or_default(),
            }
            .to_class(),
            "cursor-pointer",
            class.get(),
        )
    });
    view! {
        <button
            class=move || class.get()
            disabled=move || disabled.get().unwrap_or(false)
            aria-current=move || if is_active.get().unwrap_or(false) { Some("page") } else { None }
            data-slot="pagination-link"
            data-active=move || is_active.get()
        >
            {children.map(|c| c().into_any()).unwrap_or_else(|| ().into_any())}
        </button>
    }
}

// Previous button component
#[component]
pub fn PaginationPrevious(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] is_active: MaybeProp<bool>,
) -> impl IntoView {
    let class = Signal::derive(move || tw_merge!("gap-1 px-2.5 sm:pl-2.5", class.get()));
    view! {
        <PaginationButton
            class
            is_active
            disabled=Memo::new(move |_| !is_active.get().unwrap_or(false))
            attr:aria-label="Go to previous page"
        >
            <lucide_leptos::ChevronLeft />
            <span class="hidden sm:block">Previous</span>
        </PaginationButton>
    }
}

// Next button component
#[component]
pub fn PaginationNext(
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] is_active: MaybeProp<bool>,
) -> impl IntoView {
    let class = Signal::derive(move || tw_merge!("gap-1 px-2.5 sm:pr-2.5", class.get()));
    view! {
        <PaginationButton
            class
            is_active
            disabled=Memo::new(move |_| !is_active.get().unwrap_or(false))
            attr:aria-label="Go to next page"
        >
            <span class="hidden sm:block">Next</span>
            <lucide_leptos::ChevronRight />
        </PaginationButton>
    }
}

// Ellipsis component for indicating more pages
#[component]
pub fn PaginationEllipsis(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <span
            aria-hidden
            data-slot="pagination-ellipsis"
            class=move || {
                tw_merge!(
                    "flex size-9 items-center justify-center",
                                        class.get()
                )
            }
        >
            <lucide_leptos::Ellipsis />
        </span>
    }
}

#[derive(Debug, Clone)]
enum PageItem {
    Page(usize),
    Ellipsis(String),
}

impl PageItem {
    fn key(&self) -> String {
        match self {
            PageItem::Page(page) => format!("page-{}", page),
            PageItem::Ellipsis(id) => id.clone(),
        }
    }
}

// Page-based pagination component
#[component]
pub fn PagePagination(
    pagination_info: Signal<PaginatedInfo>,
    page_size: RwSignal<usize>,
    current_page: Signal<usize>,
    total_pages: Signal<usize>,
    #[prop(into)] on_page_change: Callback<usize>,
) -> impl IntoView {
    // Separate visual state that updates immediately
    let visual_current_page = RwSignal::new(current_page.get_untracked());

    // Sync visual state when actual current_page changes (from external sources)
    Effect::new(move |_| {
        visual_current_page.set(current_page.get());
    });

    // Create memoized signals for navigation state based on visual state
    let has_previous = Memo::new(move |_| visual_current_page.get() > 1);
    let has_next = Memo::new(move |_| {
        let current = visual_current_page.get();
        let total = total_pages.get();
        current < total && total > 1
    });
    let visible_pages = Memo::new(move |_| {
        let current = visual_current_page.get();
        let total = total_pages.get();
        get_visible_pages(current, total)
    });

    // Debounced page change with 300ms delay
    let debounce_timeout_handle: StoredValue<Option<leptos::leptos_dom::helpers::TimeoutHandle>> =
        StoredValue::new(None);

    let debounced_page_change = move |new_page: usize| {
        // Update visual state immediately
        visual_current_page.set(new_page);

        // Clear any existing timeout
        if let Some(h) = debounce_timeout_handle.get_value() {
            h.clear();
        }

        // Set new timeout for debounced action
        let h = leptos::leptos_dom::helpers::set_timeout_with_handle(
            move || {
                on_page_change.run(new_page);
            },
            std::time::Duration::from_millis(300),
        )
        .expect("set timeout for page change debounce");
        debounce_timeout_handle.set_value(Some(h));
    };

    let handle_previous = move |_| {
        let current = visual_current_page.get();
        if current > 1 {
            debounced_page_change(current - 1);
        }
    };

    let handle_next = move |_| {
        let current = visual_current_page.get_untracked();
        let total = total_pages.get_untracked();
        if current < total {
            debounced_page_change(current + 1);
        }
    };

    let handle_page_click = move |page: usize| {
        debounced_page_change(page);
    };

    // Cleanup debounce timeout on component destroy
    on_cleanup(move || {
        if let Some(Some(h)) = debounce_timeout_handle.try_get_value() {
            h.clear();
        }
    });

    let page_size_select_open = RwSignal::new(false);

    view! {
        <div class="flex flex-wrap items-center justify-between gap-4">
            <div class="text-sm text-muted-foreground">
                {move || {
                    let info = pagination_info.get();
                    let start = ((info.page - 1) * info.page_size) + 1;
                    let end = std::cmp::min(info.page * info.page_size, info.total_count);
                    if info.total_count > 0 {
                        format!("Showing {}-{} of {} results", start, end, info.total_count)
                    } else {
                        "No results found".to_string()
                    }
                }}
            </div>
            <div class="flex items-center gap-4 flex-wrap">
                <div class="flex items-center gap-2">
                    <span class="text-sm text-muted-foreground">"Rows per page:"</span>
                    <div class="w-20">
                        <Select open=page_size_select_open value=page_size>
                            <SelectTrigger class="h-8 text-sm">
                                {move || page_size.get().to_string()}
                            </SelectTrigger>
                            <SelectContent>
                                <SelectItem value=10usize>10</SelectItem>
                                <SelectItem value=20usize>20</SelectItem>
                                <SelectItem value=50usize>50</SelectItem>
                                <SelectItem value=100usize>100</SelectItem>
                            </SelectContent>
                        </Select>
                    </div>
                </div>
                <Pagination>
                    <PaginationContent>
                        // Previous button
                        <PaginationPrevious
                            is_active=Memo::new(move |_| has_previous.get())
                            on:click=handle_previous
                        />

                        // Render visible pages with ellipsis handling
                        <For
                            each=move || {
                                let current = visual_current_page.get();
                                let pages = visible_pages.get();
                                let mut page_items = Vec::new();
                                for (i, &page) in pages.iter().enumerate() {
                                    if i > 0 && page > pages[i - 1] + 1 {
                                        page_items
                                            .push(PageItem::Ellipsis(format!("ellipsis-{}", i)));
                                    }
                                    page_items.push(PageItem::Page(page));
                                }
                                page_items
                            }
                            key=|item| item.key()
                            children=move |item| {
                                match item {
                                    PageItem::Ellipsis(_) => {
                                        view! { <PaginationEllipsis /> }.into_any()
                                    }
                                    PageItem::Page(page) => {
                                        view! {
                                            <PaginationItem>
                                                <PaginationButton
                                                    is_active=Memo::new(move |_| {
                                                        page == visual_current_page.get()
                                                    })
                                                    on:click=move |_| { handle_page_click(page) }
                                                >
                                                    {page.to_string()}
                                                </PaginationButton>
                                            </PaginationItem>
                                        }
                                            .into_any()
                                    }
                                }
                            }
                        />

                        // Next button
                        <PaginationNext
                            is_active=Memo::new(move |_| has_next.get())
                            on:click=handle_next
                        />
                    </PaginationContent>
                </Pagination>
            </div>
        </div>
    }
}

// Utility functions for pagination logic (extracted for testing)
pub fn calculate_page_range(current_page: usize, total_pages: usize) -> (usize, usize) {
    let start_page = (current_page.saturating_sub(2)).max(1);
    let end_page = (current_page + 2).min(total_pages);
    (start_page, end_page)
}

pub fn get_visible_pages(current_page: usize, total_pages: usize) -> Vec<usize> {
    let (start_page, end_page) = calculate_page_range(current_page, total_pages);
    let mut pages = Vec::new();

    // Always include page 1 if not in the visible range
    if start_page > 1 {
        pages.push(1);
    }

    // Add all pages in the visible range
    pages.extend(start_page..=end_page);

    // Add last page if not already included
    if end_page < total_pages {
        pages.push(total_pages);
    }

    pages
}

pub fn should_show_ellipsis_after(current_page: usize, total_pages: usize) -> bool {
    let (_, end_page) = calculate_page_range(current_page, total_pages);
    end_page < total_pages - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_page_range_calculation() {
        // Test page 1
        assert_eq!(calculate_page_range(1, 10), (1, 3));

        // Test middle page
        assert_eq!(calculate_page_range(5, 10), (3, 7));

        // Test last page
        assert_eq!(calculate_page_range(10, 10), (8, 10));

        // Test near last page
        assert_eq!(calculate_page_range(9, 10), (7, 10));

        // Test single page
        assert_eq!(calculate_page_range(1, 1), (1, 1));

        // Test two pages
        assert_eq!(calculate_page_range(1, 2), (1, 2));
        assert_eq!(calculate_page_range(2, 2), (1, 2));
    }

    #[test]
    fn test_visible_pages() {
        // Test page 1 of 10 - should show pages 1, 2, 3, and 10
        let pages = get_visible_pages(1, 10);
        assert_eq!(pages, vec![1, 2, 3, 10]);

        // Test page 5 of 10 - should show pages 1, 3, 4, 5, 6, 7, and 10
        let pages = get_visible_pages(5, 10);
        assert_eq!(pages, vec![1, 3, 4, 5, 6, 7, 10]);

        // Test page 10 of 10 - should show pages 1, 8, 9, 10
        let pages = get_visible_pages(10, 10);
        assert_eq!(pages, vec![1, 8, 9, 10]);

        // Test page 9 of 10 - should show pages 1, 7, 8, 9, 10
        let pages = get_visible_pages(9, 10);
        assert_eq!(pages, vec![1, 7, 8, 9, 10]);

        // Test page 3 of 10 - visible range includes page 1, so no duplicate
        let pages = get_visible_pages(3, 10);
        assert_eq!(pages, vec![1, 2, 3, 4, 5, 10]);

        // Test single page
        let pages = get_visible_pages(1, 1);
        assert_eq!(pages, vec![1]);

        // Test two pages
        let pages = get_visible_pages(1, 2);
        assert_eq!(pages, vec![1, 2]);

        // Test small total pages where end_page equals total
        let pages = get_visible_pages(2, 4);
        assert_eq!(pages, vec![1, 2, 3, 4]);
    }

    #[test]
    fn test_ellipsis_logic() {
        // Should show ellipsis when there's a gap to the last page
        assert!(should_show_ellipsis_after(1, 10)); // 1,2,3 ... 10
        assert!(should_show_ellipsis_after(3, 10)); // 1,2,3,4,5 ... 10

        // Should not show ellipsis when end_page is adjacent to total
        assert!(!should_show_ellipsis_after(8, 10)); // 6,7,8,9,10 (no gap)
        assert!(!should_show_ellipsis_after(9, 10)); // 7,8,9,10 (no gap)
        assert!(!should_show_ellipsis_after(10, 10)); // 8,9,10 (no gap)

        // Edge cases
        assert!(!should_show_ellipsis_after(1, 1)); // Single page
        assert!(!should_show_ellipsis_after(1, 2)); // Two pages
        assert!(!should_show_ellipsis_after(1, 3)); // Three pages
    }

    #[test]
    fn test_page_1_always_visible() {
        // This is the key test for the bug we fixed
        let pages = get_visible_pages(1, 10);
        assert!(
            pages.contains(&1),
            "Page 1 should always be visible when current_page is 1"
        );

        let pages = get_visible_pages(1, 5);
        assert!(
            pages.contains(&1),
            "Page 1 should always be visible when current_page is 1"
        );

        let pages = get_visible_pages(1, 1);
        assert!(
            pages.contains(&1),
            "Page 1 should always be visible when current_page is 1"
        );

        // Test the specific bug scenario: when paging down to higher pages, page 1 should still be visible
        let pages = get_visible_pages(5, 10);
        assert!(
            pages.contains(&1),
            "Page 1 should be visible even when on page 5 of 10"
        );

        let pages = get_visible_pages(7, 10);
        assert!(
            pages.contains(&1),
            "Page 1 should be visible even when on page 7 of 10"
        );

        let pages = get_visible_pages(8, 10);
        assert!(
            pages.contains(&1),
            "Page 1 should be visible even when on page 8 of 10"
        );

        // Test with larger page counts
        let pages = get_visible_pages(15, 20);
        assert!(
            pages.contains(&1),
            "Page 1 should be visible even when on page 15 of 20"
        );
    }

    #[test]
    fn test_current_page_always_visible() {
        for current in 1..=10 {
            let pages = get_visible_pages(current, 10);
            assert!(
                pages.contains(&current),
                "Current page {} should always be visible in pages: {:?}",
                current,
                pages
            );
        }
    }

    #[test]
    fn test_edge_cases() {
        // Test edge case with very small total pages
        assert_eq!(get_visible_pages(1, 1), vec![1]);
        assert_eq!(get_visible_pages(1, 2), vec![1, 2]);
        assert_eq!(get_visible_pages(2, 2), vec![1, 2]);

        // Test edge case where current page is out of bounds (shouldn't happen in practice)
        let pages = get_visible_pages(0, 10); // current_page = 0
        assert!(
            !pages.is_empty(),
            "Should still return some pages even with invalid current_page"
        );

        // Test edge case with zero total pages (shouldn't happen in practice)
        let pages = get_visible_pages(1, 0);
        // This would be an invalid state, but our function should handle it gracefully
        assert!(
            pages.is_empty() || pages == vec![0],
            "Should handle zero total pages gracefully"
        );
    }
}
