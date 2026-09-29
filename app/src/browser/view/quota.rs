use super::*;
use crate::browser::clock::{Age, Countdown};

pub(super) fn render_dash(runtime: Option<&CanvasRuntime>) -> Html {
    let Some(generation) = runtime.and_then(CanvasRuntime::quota) else {
        return section_unavailable();
    };
    let presentation = crate::quota::present(generation);
    let snapshot = generation.last_good.as_deref();
    html! {
        <section
            class="section dash-overview-quota-section"
            aria-labelledby="provider-quota-heading"
            data-quota-state={presentation.condition.key()}
        >
            <div class="section-heading">
                <h2 id="provider-quota-heading">{"Provider quota"}</h2>
                if snapshot.is_some() {
                    <p class="quota-capacity" title="Weekly plan-weighted estimate across accounts · Free 0.2× · $20 plans 1× · Pro/Max 5× or explicit 20× · Anthropic without a reported plan 20×">
                        <strong>{crate::quota::capacity_summary(generation)}</strong>
                        {" weekly"}
                    </p>
                }
            </div>
            if !presentation.fresh {
                <div class="quota-status" role="status" aria-live="polite">
                    <strong>{presentation.condition.label()}</strong>
                    <span>{presentation.detail}</span>
                    if let Some(sampled_at) = presentation.sampled_at_ms {
                        <span>{"Sample age "}<Age authored={sampled_at} class={classes!("dash-overview-quota-age")} title="Age of retained successful quota data" /></span>
                    }
                </div>
            }
            {render_attempt(generation.latest_attempt.as_ref())}
            if let Some(snapshot) = snapshot {
                {render_reports(&snapshot.reports)}
                {render_accounts_without_usage(&snapshot.accounts_without_usage)}
                {render_disabled(&snapshot.disabled_credentials)}
            }
        </section>
    }
}

fn section_unavailable() -> Html {
    html! {
        <section class="section dash-overview-quota-section" aria-labelledby="provider-quota-heading" data-quota-state="unavailable">
            <div class="section-heading"><h2 id="provider-quota-heading">{"Provider quota"}</h2></div>
            <p class="quota-status" role="status">{"No admitted quota generation is available."}</p>
        </section>
    }
}

fn render_attempt(attempt: Option<&wire::QuotaAttempt>) -> Html {
    let Some(wire::QuotaAttempt::Failed {
        kind,
        exit_code,
        finished_at_ms,
    }) = attempt
    else {
        return Html::default();
    };
    let exit = exit_code
        .map(|code| format!("; exit {code}"))
        .unwrap_or_default();
    html! {
        <p class="quota-attempt" role="alert">
            {format!("Latest attempt: {}{exit}. ", crate::quota::failure_label(*kind))}
            <Age authored={i64::try_from(*finished_at_ms).unwrap_or(i64::MAX)} class={classes!("dash-overview-quota-age")} title="Age of latest failed quota attempt" />
            {" ago"}
        </p>
    }
}

fn render_reports(reports: &[wire::QuotaReport]) -> Html {
    html! {
        <section class="quota-detail-group" aria-labelledby="quota-accounts-heading">
            <h3 id="quota-accounts-heading">{"Account limits"}</h3>
            if reports.is_empty() {
                <p class="quota-empty">{"No account usage reports were returned."}</p>
            } else {
                <div class="quota-account-list">
                    {for reports.iter().enumerate().map(|(index, report)| render_report(index, report))}
                </div>
            }
        </section>
    }
}

fn render_report(index: usize, report: &wire::QuotaReport) -> Html {
    let heading = format!("quota-account-{index}");
    let label = crate::quota::account_label(&report.account);
    html! {
        <article class="quota-account" aria-labelledby={heading.clone()}>
            <header>
                <span class="quota-provider">{report.provider.clone()}</span>
                <h4 id={heading}>{label.clone()}</h4>
                {render_account_metadata(&report.account, &label)}
                <Age authored={report.fetched_at_ms} class={classes!("dash-overview-quota-age")} title="Age of this provider report" />
            </header>
            <ul class="quota-limit-list">
                {for report.limits.iter().map(render_limit)}
            </ul>
            {render_reset_credits(report.reset_credits.as_ref())}
        </article>
    }
}

/// Identity facts beyond the one already spelled by the heading.
fn render_account_metadata(account: &wire::QuotaAccount, heading: &str) -> Html {
    let facts = [
        ("Email", account.email.as_deref()),
        ("Account", account.account_id.as_deref()),
        ("Project", account.project_id.as_deref()),
        ("Org", account.org_id.as_deref()),
        ("Org name", account.org_name.as_deref()),
        ("Plan", account.plan_type.as_deref()),
    ];
    html! {
        <dl class="quota-account-metadata">
            {for facts.into_iter().filter_map(|(label, value)| {
                let value = value.filter(|value| *value != heading)?;
                Some(html! { <div><dt>{label}</dt><dd>{value.to_owned()}</dd></div> })
            })}
        </dl>
    }
}

fn render_limit(limit: &wire::QuotaLimit) -> Html {
    let status = limit
        .status
        .as_deref()
        .filter(|status| !status.eq_ignore_ascii_case("ok"));
    let used = crate::quota::limit_remaining_fraction(limit).map(|remaining| 1.0 - remaining);
    html! {
        <li class="quota-limit" data-limit-status={status.map(str::to_owned)}>
            <strong>
                {limit.label.clone()}
                if let Some(status) = status {
                    <span class="quota-limit-status">{status.to_owned()}</span>
                }
            </strong>
            {render_meter(used)}
            <span class="quota-limit-used">{usage_text(&limit.amount)}</span>
            <span class="quota-limit-reset">{render_reset(limit.window.as_ref())}</span>
        </li>
    }
}

/// Used share drawn as SVG geometry: the canvas CSP refuses inline styles.
fn render_meter(used: Option<f64>) -> Html {
    let Some(used) = used else {
        return html! { <span class="quota-meter" aria-hidden="true"></span> };
    };
    html! {
        <svg class="quota-meter" viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true">
            <rect class="quota-meter-track" width="100" height="6" rx="3" />
            <rect class="quota-meter-used" width={format!("{:.1}", used * 100.0)} height="6" rx="3" />
        </svg>
    }
}

/// The amount the operator acts on; a percent limit is always 100, so it and
/// the complementary remainder are not repeated.
fn usage_text(amount: &wire::QuotaAmount) -> String {
    let percent = amount.unit.eq_ignore_ascii_case("percent");
    let quantity = |value: &str| {
        if percent {
            format!("{value}%")
        } else {
            format!("{value} {}", amount.unit)
        }
    };
    match (
        amount.used.as_deref(),
        amount.remaining.as_deref(),
        amount.limit.as_deref(),
    ) {
        (Some(used), _, Some(limit)) if !percent => format!("{used} / {}", quantity(limit)),
        (Some(used), _, _) => format!("{} used", quantity(used)),
        (None, Some(remaining), Some(limit)) if !percent => {
            format!("{remaining} / {} left", quantity(limit))
        }
        (None, Some(remaining), _) => format!("{} left", quantity(remaining)),
        (None, None, _) => "usage unknown".to_owned(),
    }
}

fn render_reset(window: Option<&wire::QuotaWindow>) -> Html {
    let label = window.and_then(|window| window.reset_label.clone());
    match window.and_then(|window| window.resets_at) {
        Some(at) => {
            let exact = iso_time(at);
            let title = label.map_or_else(|| exact.clone(), |label| format!("{label} · {exact}"));
            html! { <>{"resets "}<Countdown expiry={at} class={classes!("dash-overview-quota-age")} title={title} /></> }
        }
        None => html! { {label.unwrap_or_default()} },
    }
}

fn iso_time(milliseconds: i64) -> String {
    js_sys::Date::new(&wasm_bindgen::JsValue::from_f64(milliseconds as f64))
        .to_iso_string()
        .as_string()
        .unwrap_or_else(|| milliseconds.to_string())
}

fn render_reset_credits(credits: Option<&wire::QuotaResetCredits>) -> Html {
    let Some(credits) = credits else {
        return Html::default();
    };
    html! {
        <p class="quota-reset-credits">
            <strong>{"Reset credits"}</strong>
            <span>{format!("{} available", credits.available_count)}</span>
            if let Some(redeemable) = credits.redeemable_count {
                <span>{format!("{redeemable} redeemable")}</span>
            }
            {for credits.credits.iter().filter_map(|credit| {
                let title = credit.title.as_ref().or(credit.program.as_ref());
                let text = match (title, credit.remaining_count) {
                    (Some(title), Some(count)) => format!("{title} ({count} remaining)"),
                    (Some(title), None) => title.clone(),
                    (None, Some(count)) => format!("{count} remaining"),
                    (None, None) => return None,
                };
                Some(html! { <span class="quota-reset-credit">{text}</span> })
            })}
        </p>
    }
}

fn render_accounts_without_usage(accounts: &[wire::QuotaAccountWithoutUsage]) -> Html {
    html! {
        <section class="quota-detail-group" aria-labelledby="quota-without-usage-heading">
            <h3 id="quota-without-usage-heading">{"Accounts lacking usage"}</h3>
            if accounts.is_empty() {
                <p class="quota-empty">{"None."}</p>
            } else {
                <ul class="quota-simple-list">{for accounts.iter().map(|entry| html! {
                    <li><strong>{entry.provider.clone()}</strong><span>{crate::quota::account_label(&entry.account)}</span><span>{entry.credential_type.clone()}</span></li>
                })}</ul>
            }
        </section>
    }
}

fn render_disabled(credentials: &[wire::QuotaDisabledCredential]) -> Html {
    html! {
        <section class="quota-detail-group" aria-labelledby="quota-disabled-heading">
            <h3 id="quota-disabled-heading">{"Disabled credentials"}</h3>
            if credentials.is_empty() {
                <p class="quota-empty">{"None."}</p>
            } else {
                <ul class="quota-simple-list">{for credentials.iter().map(|entry| html! {
                    <li><strong>{entry.provider.clone()}</strong><span>{crate::quota::account_label(&entry.account)}</span><span>{entry.credential_type.clone()}</span></li>
                })}</ul>
            }
        </section>
    }
}
