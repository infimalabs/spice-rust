//! Conditional quantiles and observed spans, never reconstructed activity.

use super::{Publication, Quantity, Result, escape, figure, number, opening, short};
use std::path::Path;

pub(in super::super) fn quantiles(
    quantity: &Quantity,
    public: &Path,
    name: &str,
) -> Result<String> {
    let maximum = quantity
        .values
        .iter()
        .map(|row| row.value)
        .fold(1.0_f64, f64::max);
    let ceiling = (maximum / 100.0).ceil() * 100.0;
    let mut groups = quantity.group_samples.iter().collect::<Vec<_>>();
    groups.sort_by_key(|(group, _)| (window_order(group), group.as_str()));
    let height = 64.0 + groups.len() as f64 * 26.0;
    let mut svg = opening(height, &quantity.name, name);
    svg.push_str("<text x=\"0\" y=\"16\">Window / phase</text><text x=\"230\" y=\"16\">n</text><text x=\"644\" y=\"16\" text-anchor=\"end\">Median</text>");
    for index in 0..=4 {
        let x = 275.0 + index as f64 * 75.0;
        svg.push_str(&format!("<path d=\"M{x} 35V{}\" stroke=\"#e3e5e7\"/><text x=\"{x}\" y=\"30\" text-anchor=\"middle\" style=\"font-size:11px\">{}</text>",height-12.0,number(ceiling*index as f64/4.0)));
    }
    for (index, (group, count)) in groups.into_iter().enumerate() {
        let quantiles = ["median", "p90", "p95"].map(|suffix| {
            let label = format!("{group} / {suffix}");
            quantity
                .values
                .iter()
                .find(|row| row.label == label)
                .map(|row| row.value)
                .ok_or("sample group lacks its quantile")
        });
        let [median, p90, p95] = quantiles;
        let (median, p90, p95) = (median?, p90?, p95?);
        let x = |value: f64| 275.0 + value / ceiling * 300.0;
        let y = 48.0 + index as f64 * 26.0;
        svg.push_str(&format!("<text x=\"0\" y=\"{}\">{}</text><text x=\"250\" y=\"{}\" text-anchor=\"end\">{count}</text><path d=\"M{} {y}H{}\" stroke=\"#c05629\" stroke-width=\"2\"/><circle cx=\"{}\" cy=\"{y}\" r=\"4\" fill=\"#c05629\"/><path d=\"M{} {}v10M{} {}v14\" stroke=\"#c05629\" stroke-width=\"2\"/><text x=\"644\" y=\"{}\" text-anchor=\"end\">{}</text>",y+4.0,escape(&short(group)),y+4.0,x(median),x(p95),x(median),x(p90),y-5.0,x(p95),y-7.0,y+4.0,number(median)));
    }
    svg.push_str("</svg>");
    figure(
        public,
        name,
        svg,
        &format!(
            "{} · minutes, linear axis from zero. Dot: median; short tick: p90; end tick: p95. This is a quantile interval, not a confidence interval or full distribution. <a href=\"tables.html#{}\">All quantiles, coverage and exclusions</a>.",
            quantity.id, quantity.id
        ),
    )
}

fn window_order(group: &str) -> u8 {
    if group.starts_with("Before") {
        0
    } else if group.starts_with("After blocking") {
        1
    } else {
        2
    }
}

pub(in super::super) fn calendar(quantity: &Quantity, public: &Path, name: &str) -> Result<String> {
    let maximum = quantity
        .values
        .iter()
        .map(|row| row.value)
        .fold(1.0_f64, f64::max);
    let ceiling = (maximum / 40.0).ceil() * 40.0;
    let mut svg = opening(225.0, "Selected completions by UTC day", name);
    for index in 0..=3 {
        let y = 180.0 - index as f64 * 50.0;
        svg.push_str(&format!("<path d=\"M35 {y}H650\" stroke=\"#dde1e3\"/><text x=\"28\" y=\"{}\" text-anchor=\"end\">{}</text>",y+4.0,number(ceiling*index as f64/3.0)));
    }
    let step = 610.0 / quantity.values.len().max(1) as f64;
    for (index, row) in quantity.values.iter().enumerate() {
        let x = 39.0 + index as f64 * step;
        let height = row.value / ceiling * 150.0;
        svg.push_str(&format!("<rect x=\"{x:.2}\" y=\"{:.2}\" width=\"{:.2}\" height=\"{height:.2}\" fill=\"#b34d7a\"><title>{}: {}</title></rect>",180.0-height,(step-3.0).max(1.0),escape(&row.label),number(row.value)));
        if index % 4 == 0 || index + 1 == quantity.values.len() {
            svg.push_str(&format!("<text x=\"{}\" y=\"200\" text-anchor=\"middle\" style=\"font-size:11px\">{}</text>",x+step/2.0,escape(&row.label[5..])));
        }
    }
    svg.push_str("<text x=\"35\" y=\"17\">Selected deliveries / day</text></svg>");
    figure(
        public,
        name,
        svg,
        "Q17 · UTC days; one linear axis from zero. Only selected deliveries, not filings or claims. A zero day remains on the calendar. <a href=\"tables.html#Q17\">Daily counts</a>.",
    )
}

fn day_number(day: &str) -> Result<i64> {
    Ok(format!("{day}T00:00:00Z")
        .parse::<jiff::Timestamp>()?
        .as_second()
        / 86_400)
}

pub(in super::super) fn sessions(p: &Publication, public: &Path, name: &str) -> Result<String> {
    let lanes = p
        .continuity
        .lanes
        .iter()
        .filter(|lane| lane.admitted_episodes > 0)
        .collect::<Vec<_>>();
    let spans = p
        .continuity
        .sessions
        .iter()
        .filter(|span| lanes.iter().any(|lane| lane.lane == span.lane))
        .collect::<Vec<_>>();
    let start = spans
        .iter()
        .map(|span| day_number(&span.first_day))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .min()
        .ok_or("no admitted-lane session spans")?;
    let end = spans
        .iter()
        .map(|span| day_number(&span.last_day))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .max()
        .ok_or("no admitted-lane session spans")?;
    let extent = (end - start).max(1) as f64;
    let x = |day: i64| 82.0 + (day - start) as f64 / extent * 490.0;
    let height = 76.0 + lanes.len() as f64 * 19.0;
    let mut svg = opening(
        height,
        "First and last retained provider-session observations",
        name,
    );
    svg.push_str("<text x=\"0\" y=\"16\">Lane</text><text x=\"82\" y=\"16\">Observed span · endpoint ticks are observations</text><text x=\"650\" y=\"16\" text-anchor=\"end\">Sessions</text>");
    for offset in (0..=end - start).step_by(7) {
        let tick = x(start + offset);
        let date = jiff::Timestamp::from_second((start + offset) * 86_400)?.to_string();
        svg.push_str(&format!("<path d=\"M{tick} 28V{}\" stroke=\"#eee9ed\"/><text x=\"{tick}\" y=\"{}\" text-anchor=\"middle\" style=\"font-size:11px\">{}</text>",height-38.0,height-18.0,&date[5..10]));
    }
    for (index, lane) in lanes.iter().enumerate() {
        let y = 40.0 + index as f64 * 19.0;
        svg.push_str(&format!("<path d=\"M82 {y}H572\" stroke=\"#e3e5e7\"/><text x=\"0\" y=\"{}\">{}</text><text x=\"650\" y=\"{}\" text-anchor=\"end\">{}</text>",y+4.0,lane.lane,y+4.0,lane.provider_sessions));
        for span in spans.iter().filter(|span| span.lane == lane.lane) {
            let first = x(day_number(&span.first_day)?);
            let last = x(day_number(&span.last_day)?);
            svg.push_str(&format!("<path d=\"M{first:.2} {y}H{last:.2}\" stroke=\"#b34d7a\" stroke-width=\"5\"/><path d=\"M{first:.2} {}v12M{last:.2} {}v12\" stroke=\"#b34d7a\" stroke-width=\"1.5\"><title>{} · {} to {} · {} declarations</title></path>",y-6.0,y-6.0,span.id,span.first_day,span.last_day,span.declarations));
        }
    }
    let first_day = spans
        .iter()
        .map(|span| span.first_day.as_str())
        .min()
        .ok_or("no span start")?;
    let last_day = spans
        .iter()
        .map(|span| span.last_day.as_str())
        .max()
        .ok_or("no span end")?;
    svg.push_str(&format!("<text x=\"82\" y=\"{}\" style=\"font-size:11px\">{first_day} — {last_day} · UTC days</text></svg>",height-2.0));
    figure(
        public,
        name,
        svg,
        &format!(
            "{} lanes with in-window admissions; {} observed provider-session spans on those lanes. Same-day endpoints can coincide. Lines join observations, not established activity. Zero-admission lanes are omitted. <a href=\"tables.html#continuity\">Every span, date, declaration and lifecycle count</a>.",
            lanes.len(),
            spans.len()
        ),
    )
}

pub(in super::super) fn windows(
    p: &Publication,
    q: &Quantity,
    public: &Path,
    name: &str,
) -> Result<String> {
    const MIN_PHASE_LABEL_WIDTH: f64 = 38.0;
    let integrated_day = |id: &str| -> Result<&str> {
        p.cases
            .iter()
            .find(|case| case.id == id)
            .and_then(|case| case.steps.iter().find(|step| step.action == "phase done"))
            .and_then(|step| step.hour.split_once(' ').map(|(day, _)| day))
            .ok_or_else(|| format!("case {id} lacks an integrated boundary").into())
    };
    let dates = [
        p.first_delivery_day.as_str(),
        integrated_day("E05")?,
        integrated_day("E03")?,
        p.horizon_day.as_str(),
    ];
    let days = dates.map(day_number);
    let [start, policy, refill, end] = days;
    let days = [start?, policy?, refill?, end?];
    let x = |day: i64| 15.0 + (day - days[0]) as f64 / (days[3] - days[0]).max(1) as f64 * 630.0;
    let mut svg = opening(
        355.0,
        "Recorded integration boundaries and admitted phase mix",
        name,
    );
    svg.push_str("<text x=\"0\" y=\"18\">Day-coarsened observation windows · integration ≠ activation</text>");
    for (index, title) in ["Pre-policy", "Policy to refill", "Post-refill"]
        .into_iter()
        .enumerate()
    {
        let left = x(days[index]);
        let right = x(days[index + 1]);
        svg.push_str(&format!("<rect x=\"{left}\" y=\"64\" width=\"{}\" height=\"38\" fill=\"{}\"/><text x=\"{}\" y=\"88\" text-anchor=\"middle\" style=\"fill:#fff\">{title}</text>",right-left,["#8e3e61","#a74571","#b34d7a"][index],(left+right)/2.0));
    }
    for (index, day) in days.into_iter().enumerate() {
        let tick = x(day);
        svg.push_str(&format!("<path d=\"M{tick} 44V108\" stroke=\"#b34d7a\"/><text x=\"{tick}\" y=\"{}\" text-anchor=\"{}\" style=\"font-size:12px\">{}</text>",if index%2==0 {38}else{126},if index==0 {"start"}else if index==3 {"end"}else{"middle"},dates[index]));
    }
    let maximum = q.values.iter().map(|row| row.value).fold(1.0_f64, f64::max);
    svg.push_str("<text x=\"0\" y=\"157\">Phase composition · counts, not calendar width</text>");
    for (index, window) in [
        "Before blocking-policy integration",
        "After blocking-policy integration",
        "After materialization integration",
    ]
    .into_iter()
    .enumerate()
    {
        let y = 190.0 + index as f64 * 50.0;
        svg.push_str(&format!(
            "<text x=\"0\" y=\"{y}\">{}</text>",
            ["Pre-policy", "Policy to refill", "Post-refill"][index]
        ));
        let mut left = 185.0;
        for phase in ["implementation", "plan", "todo"] {
            let label = format!("{window} / {phase}");
            let count = q
                .values
                .iter()
                .find(|row| row.label == label)
                .map_or(0.0, |row| row.value);
            if count == 0.0 {
                continue;
            }
            let width = count / maximum * 300.0;
            svg.push_str(&format!("<rect x=\"{left}\" y=\"{}\" width=\"{width}\" height=\"25\" fill=\"{}\"><title>{phase}: {count}</title></rect>",y-16.0,if phase=="todo" {"#b34d7a"}else{"#ecd7e1"}));
            if width > MIN_PHASE_LABEL_WIDTH {
                svg.push_str(&format!("<text x=\"{}\" y=\"{}\" text-anchor=\"middle\" style=\"fill:{}\">{count}</text>",left+width/2.0,y+2.0,if phase=="todo" {"#fff"}else{"#1b2025"}));
            }
            left += width;
        }
    }
    svg.push_str(
        "<text x=\"185\" y=\"340\">Light: plan / legacy implementation · dark: todo</text></svg>",
    );
    figure(
        public,
        name,
        svg,
        "E05 / E03 · Top: UTC calendar width between recorded, day-coarsened boundaries. Bottom: eligible phase-admission counts on one separate common length scale. The five legacy implementation episodes remain in the pre-policy total. These integrations do not date running-appliance activation. <a href=\"tables.html#Q07\">Exact phase mix</a>.",
    )
}
