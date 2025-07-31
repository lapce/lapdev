use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;

// Pagination component with optional debouncing
#[component]
pub fn SimplePagination(
    current_page: Signal<usize>,
    total_pages: Signal<usize>,
    #[prop(into)] on_page_change: Callback<usize>,
    #[prop(optional)] debounce_ms: Option<u32>,
) -> impl IntoView {
    // Create a local UI state that updates immediately
    let ui_current_page = RwSignal::new(current_page.get());
    let timeout_handle = RwSignal::new(None::<i32>);

    // Sync UI state with the external signal when it changes
    Effect::new(move || {
        ui_current_page.set(current_page.get());
    });

    // Helper function to execute page change with debouncing
    let execute_page_change = {
        let on_page_change = on_page_change.clone();
        move |new_page: usize| {
            // IMMEDIATELY update the UI state
            ui_current_page.set(new_page);
            
            if let Some(debounce_delay) = debounce_ms {
                // Cancel any existing timeout
                if let Some(handle) = timeout_handle.get() {
                    web_sys::window().unwrap().clear_timeout_with_handle(handle);
                }
                
                // Set new timeout to fire the actual callback
                let timeout_callback = Closure::wrap(Box::new({
                    let timeout_handle = timeout_handle.clone();
                    let on_page_change = on_page_change.clone();
                    move || {
                        timeout_handle.set(None);
                        on_page_change.run(new_page);
                    }
                }) as Box<dyn Fn()>);
                
                let handle = web_sys::window()
                    .unwrap()
                    .set_timeout_with_callback_and_timeout_and_arguments_0(
                        timeout_callback.as_ref().unchecked_ref(),
                        debounce_delay as i32,
                    )
                    .unwrap();
                    
                timeout_handle.set(Some(handle));
                timeout_callback.forget();
            } else {
                // No debouncing - fire immediately
                on_page_change.run(new_page);
            }
        }
    };

    let handle_previous = {
        let execute_page_change = execute_page_change.clone();
        move |_| {
            let current = ui_current_page.get();
            if current > 1 {
                execute_page_change(current - 1);
            }
        }
    };

    let handle_next = {
        let execute_page_change = execute_page_change.clone();
        move |_| {
            let current = ui_current_page.get();
            let total = total_pages.get();
            if current < total {
                execute_page_change(current + 1);
            }
        }
    };

    let handle_page_click = {
        let execute_page_change = execute_page_change.clone();
        move |page: usize| {
            move |_| {
                execute_page_change(page);
            }
        }
    };

    view! {
        {move || {
            let current = ui_current_page.get();
            let total = total_pages.get();

            if total <= 1 {
                return view! { <div></div> }.into_any();
            }

            // Get visible pages using the corrected logic
            let visible_pages = get_visible_pages(current, total);

            view! {
                <nav class="flex items-center justify-center space-x-1" aria-label="Pagination">
                    // Previous button
                    <button
                        class=format!(
                            "px-3 py-2 text-sm font-medium text-gray-500 bg-white border border-gray-300 rounded-l-md hover:bg-gray-50 {}",
                            if current == 1 { "cursor-not-allowed opacity-50" } else { "" }
                        )
                        disabled=current == 1
                        on:click=handle_previous
                    >
                        "Previous"
                    </button>

                    // Render visible pages with proper ellipsis handling
                    {visible_pages
                        .iter()
                        .enumerate()
                        .map(|(i, &page)| {
                            let mut elements = Vec::new();

                            // Add ellipsis before this page if there's a gap
                            if i > 0 && page > visible_pages[i-1] + 1 {
                                elements.push(view! {
                                    <span class="px-3 py-2 text-sm font-medium text-gray-500 bg-white border-t border-b border-gray-300">
                                        "..."
                                    </span>
                                }.into_any());
                            }

                            // Add the page button
                            elements.push(view! {
                                <button
                                    class=format!(
                                        "px-3 py-2 text-sm font-medium border-t border-b border-gray-300 {}",
                                        if page == current {
                                            "bg-primary text-primary-foreground"
                                        } else {
                                            "text-gray-700 bg-white hover:bg-gray-50"
                                        }
                                    )
                                    on:click=handle_page_click(page)
                                >
                                    {page.to_string()}
                                </button>
                            }.into_any());

                            elements
                        })
                        .flatten()
                        .collect::<Vec<_>>()
                    }

                    // Next button
                    <button
                        class=format!(
                            "px-3 py-2 text-sm font-medium text-gray-500 bg-white border border-gray-300 rounded-r-md hover:bg-gray-50 {}",
                            if current == total { "cursor-not-allowed opacity-50" } else { "" }
                        )
                        disabled=current == total
                        on:click=handle_next
                    >
                        "Next"
                    </button>
                </nav>
            }.into_any()
        }}
    }
}

#[component]
pub fn Pagination(
    #[prop(into, optional)] class: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    // let pagination_class = PaginationClass;

    view! {
        <nav
            role="navigation"
            aria-label="pagination"
            // class=move || format!("{} {}", pagination_class.to_class(), class.get().unwrap_or_default())
        >
            {children()}
        </nav>
    }
}

#[component]
pub fn PaginationContent(
    #[prop(into, optional)] class: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    // let content_class = PaginationContentClass;

    view! {
        <ul>
        // class=move || format!("{} {}", content_class.to_class(), class.get().unwrap_or_default())>
            {children()}
        </ul>
    }
}

#[component]
pub fn PaginationItem(
    #[prop(into, optional)] class: MaybeProp<String>,
    children: Children,
) -> impl IntoView {
    view! {
        <li class=move || class.get().unwrap_or_default()>
            {children()}
        </li>
    }
}

#[component]
pub fn PaginationLink(
    // #[prop(into)] href: MaybeSignal<String>,
    // #[prop(default = PaginationItemVariant::Default)] variant: PaginationItemVariant,
    // #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(into, optional)] size: MaybeProp<String>,
    #[prop(default = false)] disabled: bool,
    children: Children,
) -> impl IntoView {
    // let item_class = PaginationItemClass { variant };

    view! {
        <a
            // href=move || href.get()
            // class=move || {
            //     format!(
            //         "{} {} {}",
            //         item_class.to_class(),
            //         if disabled { "pointer-events-none opacity-50" } else { "" },
            //         class.get().unwrap_or_default()
            //     )
            // }
            aria-disabled=disabled
        >
            {children()}
        </a>
    }
}

#[component]
pub fn PaginationPrevious(
    #[prop(into, optional)] href: MaybeProp<String>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(default = false)] disabled: bool,
) -> impl IntoView {
    // let href = href.unwrap_or_else(|| MaybeProp::Static("#".to_string()));

    view! {
        <PaginationLink
            // href=href
            // variant=PaginationItemVariant::Ghost
            // class=move || format!("gap-1 pl-2.5 {}", class.get().unwrap_or_default())
            disabled=disabled
        >
            <ChevronLeft class="size-4" />
            <span>"Previous"</span>
        </PaginationLink>
    }
}

#[component]
pub fn PaginationNext(
    #[prop(into, optional)] href: MaybeProp<String>,
    #[prop(into, optional)] class: MaybeProp<String>,
    #[prop(default = false)] disabled: bool,
) -> impl IntoView {
    // let href = href.unwrap_or_else(|| MaybeProp::Static("#".to_string()));

    view! {
        <PaginationLink
            // href=href
            // variant=PaginationItemVariant::Ghost
            // class=move || format!("gap-1 pr-2.5 {}", class.get().unwrap_or_default())
            disabled=disabled
        >
            <span>"Next"</span>
            <ChevronRight class="size-4" />
        </PaginationLink>
    }
}

#[component]
pub fn PaginationEllipsis(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <span
            aria-hidden="true"
            class=move || format!("flex h-9 w-9 items-center justify-center {}", class.get().unwrap_or_default())
        >
            <MoreHorizontal class="size-4" />
            <span class="sr-only">"More pages"</span>
        </span>
    }
}

// Icon components - these would typically be imported from an icon library
#[component]
pub fn ChevronLeft(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <svg
            class=move || class.get().unwrap_or_default()
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
            xmlns="http://www.w3.org/2000/svg"
        >
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M15 19l-7-7 7-7" />
        </svg>
    }
}

#[component]
pub fn ChevronRight(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <svg
            class=move || class.get().unwrap_or_default()
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
            xmlns="http://www.w3.org/2000/svg"
        >
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M9 5l7 7-7 7" />
        </svg>
    }
}

#[component]
pub fn MoreHorizontal(#[prop(into, optional)] class: MaybeProp<String>) -> impl IntoView {
    view! {
        <svg
            class=move || class.get().unwrap_or_default()
            fill="none"
            stroke="currentColor"
            viewBox="0 0 24 24"
            xmlns="http://www.w3.org/2000/svg"
        >
            <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M5 12h.01M12 12h.01M19 12h.01" />
        </svg>
    }
}

// Utility component for building complete pagination
#[component]
pub fn CompletePagination(
    current_page: i32,
    total_pages: i32,
    #[prop(into)] on_page_change: Callback<i32>,
    #[prop(into, optional)] base_url: MaybeProp<String>,
) -> impl IntoView {
    // let base_url = base_url.unwrap_or_else(|| MaybeProp::Static("".to_string()));

    // let get_page_url = move |page: i32| {
    //     let base = base_url.get();
    //     if base.is_empty() {
    //         "#".to_string()
    //     } else {
    //         format!("{}?page={}", base, page)
    //     }
    // };

    // let handle_page_click = move |page: i32| {
    //     move |_| {
    //         on_page_change.run(page);
    //     }
    // };

    // Calculate visible page range
    // let start_page = (current_page - 2).max(1);
    // let end_page = (current_page + 2).min(total_pages);

    view! {}
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
