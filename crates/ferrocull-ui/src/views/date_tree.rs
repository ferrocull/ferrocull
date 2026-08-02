use std::collections::{BTreeMap, BTreeSet};

use chrono::{Datelike, Local};
use ferrocull_core::media::{DateSelection, Item};
use iced::{
    Element, Fill,
    widget::{Space, button, column, container, lazy, row, text},
};

use crate::{messages::filters::Message, styles, theme::spacing};

type DateCounts = BTreeMap<i32, BTreeMap<u32, BTreeMap<u32, usize>>>;

fn date_counts(items: &[Item]) -> DateCounts {
    let mut counts: DateCounts = BTreeMap::new();

    for item in items {
        let local = item.capture_time.second.with_timezone(&Local);
        *counts
            .entry(local.year())
            .or_default()
            .entry(local.month())
            .or_default()
            .entry(local.day())
            .or_default() += 1;
    }

    counts
}

const fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => unreachable!(),
    }
}

fn expand_icon(expanded: bool) -> iced::widget::Text<'static> {
    if expanded {
        crate::icons::chevron_expanded()
    } else {
        crate::icons::chevron_collapsed()
    }
}

fn ordered_keys<K: Copy, V>(map: &BTreeMap<K, V>, ascending: bool) -> Vec<K> {
    if ascending {
        map.keys().copied().collect()
    } else {
        map.keys().rev().copied().collect()
    }
}

/// Cache key for `lazy` — stores actual values for exact equality (no hash collisions).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    /// Monotonic counter — bumped on any item change. O(1) instead of O(n) hash.
    item_version: u64,
    selected_date: Option<DateSelection>,
    expanded_years: BTreeSet<i32>,
    expanded_months: BTreeSet<(i32, u32)>,
    ascending: bool,
}

pub(crate) fn date_tree<'a>(
    items: &'a [Item],
    item_version: u64,
    selected_date: Option<DateSelection>,
    expanded_years: &'a BTreeSet<i32>,
    expanded_months: &'a BTreeSet<(i32, u32)>,
    ascending: bool,
) -> Element<'a, Message> {
    let key = CacheKey {
        item_version,
        selected_date,
        expanded_years: expanded_years.clone(),
        expanded_months: expanded_months.clone(),
        ascending,
    };

    let exp_years = expanded_years.clone();
    let exp_months = expanded_months.clone();

    lazy(key, move |_| {
        let counts = date_counts(items);

        if counts.is_empty() {
            return Element::from(
                container(text("No dates available").size(12)).padding(spacing::SM),
            );
        }

        let mut rows: Vec<Element<'static, Message>> = Vec::new();

        for year in ordered_keys(&counts, ascending) {
            let months = &counts[&year];
            let year_total: usize = months.values().flat_map(BTreeMap::values).sum();
            let is_expanded = exp_years.contains(&year);
            let is_selected = selected_date == Some(DateSelection::year_only(year));

            rows.push(
                button(
                    row![
                        button(expand_icon(is_expanded).size(10))
                            .padding([2, 4])
                            .style(button::text)
                            .on_press(Message::YearExpanded(year)),
                        text(format!("{year} ({year_total})")).size(12),
                    ]
                    .align_y(iced::Alignment::Center),
                )
                .padding([2, 6])
                .width(Fill)
                .style(styles::date_tree_item(is_selected))
                .on_press(Message::DateToggled(DateSelection::year_only(year)))
                .into(),
            );

            if is_expanded {
                push_month_rows(
                    &mut rows,
                    months,
                    year,
                    selected_date,
                    &exp_months,
                    ascending,
                );
            }
        }

        let sort_toggle = button(
            if ascending {
                crate::icons::sort_ascending()
            } else {
                crate::icons::sort_descending()
            }
            .size(11),
        )
            .padding([2, 6])
            .style(styles::secondary_button)
            .on_press(Message::DateSortToggled);
        let header = container(
            row![
                text("Filter by Date").size(13),
                Space::new().width(Fill),
                sort_toggle,
            ]
            .align_y(iced::Alignment::Center),
        )
        .padding([spacing::SM, 0.0])
        .width(Fill);

        Element::from(column![header, column(rows)].spacing(spacing::XS))
    })
    .into()
}

fn push_month_rows(
    rows: &mut Vec<Element<'static, Message>>,
    months: &BTreeMap<u32, BTreeMap<u32, usize>>,
    year: i32,
    selected_date: Option<DateSelection>,
    exp_months: &BTreeSet<(i32, u32)>,
    ascending: bool,
) {
    for month in ordered_keys(months, ascending) {
        let days = &months[&month];
        let month_count: usize = days.values().sum();
        let is_expanded = exp_months.contains(&(year, month));
        let is_selected = selected_date == Some(DateSelection::year_month(year, month));

        rows.push(
            button(
                row![
                    Space::new().width(16.0),
                    button(expand_icon(is_expanded).size(10))
                        .padding([2, 4])
                        .style(button::text)
                        .on_press(Message::MonthExpanded(year, month)),
                    text(format!("{} ({month_count})", month_name(month))).size(11),
                ]
                .align_y(iced::Alignment::Center),
            )
            .padding([2, 6])
            .width(Fill)
            .style(styles::date_tree_item(is_selected))
            .on_press(Message::DateToggled(DateSelection::year_month(year, month)))
            .into(),
        );

        if is_expanded {
            for day in ordered_keys(days, ascending) {
                let count = days[&day];
                let is_day_selected =
                    selected_date == Some(DateSelection::year_month_day(year, month, day));

                rows.push(
                    button(
                        row![
                            Space::new().width(44.0),
                            text(format!("{day} ({count})")).size(11),
                        ]
                        .align_y(iced::Alignment::Center),
                    )
                    .padding([2, 6])
                    .width(Fill)
                    .style(styles::date_tree_item(is_day_selected))
                    .on_press(Message::DateToggled(DateSelection::year_month_day(
                        year, month, day,
                    )))
                    .into(),
                );
            }
        }
    }
}
