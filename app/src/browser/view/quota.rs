use super::*;
use crate::browser::clock::{Age, Countdown};

pub(super) fn render_dash(runtime: Option<&AppRuntime>) -> Html {
    let Some(generation) = runtime.and_then(AppRuntime::quota) else {
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
                    <p class="quota-capacity" title="Weekly plan-weighted estimate across accounts · Free 0.2× · named $20 plans 1× · explicit 5× and OpenAI Pro Lite 5× · Pro/Max 20× by default">
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
                {render_reports(snapshot)}
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

fn render_reports(snapshot: &wire::QuotaSnapshot) -> Html {
    let accents = crate::quota::weekly_quota_stack(snapshot)
        .map(|stack| stack.report_accents(snapshot.reports.len()));
    html! {
        <section class="quota-detail-group" aria-labelledby="quota-accounts-heading">
            <h3 id="quota-accounts-heading">{"Account limits"}</h3>
            if snapshot.reports.is_empty() {
                <p class="quota-empty quota-account-empty">{"No account usage reports were returned."}</p>
            } else {
                <div class="quota-account-list">
                    {for snapshot.reports.iter().enumerate().map(|(index, report)| {
                        let accent = accents
                            .as_deref()
                            .and_then(|accents| accents[index]);
                        render_report(index, report, accent)
                    })}
                </div>
            }
        </section>
    }
}

fn render_report(index: usize, report: &wire::QuotaReport, accent: Option<usize>) -> Html {
    let heading = format!("quota-account-{index}");
    let label = crate::quota::account_label(&report.account);
    html! {
        <article
            class="quota-account"
            aria-labelledby={heading.clone()}
            data-accent={accent.map(|accent| (accent % crate::watchband::ACCENTS).to_string())}
        >
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
    let remaining = crate::quota::limit_remaining_fraction(limit);
    html! {
        <li class="quota-limit" data-limit-status={status.map(str::to_owned)}>
            <strong>
                {limit.label.clone()}
                if let Some(status) = status {
                    <span class="quota-limit-status">{status.to_owned()}</span>
                }
            </strong>
            {render_meter(remaining)}
            <span class="quota-limit-amount">{crate::quota::amount_text(&limit.amount)}</span>
            <span class="quota-limit-reset">{render_reset(limit.window.as_ref())}</span>
        </li>
    }
}

/// Remaining share drawn as SVG geometry (the app CSP refuses inline styles),
/// anchored at the bar's end beside the amount, so usage empties it toward
/// that end. The amount follows the bar in row order.
fn render_meter(remaining: Option<f64>) -> Html {
    let Some(remaining) = remaining else {
        return html! { <span class="quota-meter" aria-hidden="true"></span> };
    };
    let width = remaining * 100.0;
    html! {
        <svg class="quota-meter" viewBox="0 0 100 6" preserveAspectRatio="none" aria-hidden="true">
            <rect class="quota-meter-track" width="100" height="6" rx="3" />
            <rect
                class="quota-meter-left"
                x={format!("{:.1}", 100.0 - width)}
                width={format!("{width:.1}")}
                height="6"
                rx="3"
            />
        </svg>
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

fn iso_millis(value: &str) -> Option<i64> {
    let milliseconds = js_sys::Date::parse(value);
    milliseconds.is_finite().then_some(milliseconds as i64)
}

fn render_reset_credit(credit: &wire::QuotaResetCredit) -> Option<Html> {
    let title = credit.title.as_ref().or(credit.program.as_ref());
    let text = match (title, credit.remaining_count) {
        (Some(title), Some(count)) => format!("{title} ({count} remaining)"),
        (Some(title), None) => title.clone(),
        (None, Some(count)) => format!("{count} remaining"),
        (None, None) => return None,
    };
    let expiry = credit
        .expires_at
        .as_deref()
        .and_then(|exact| iso_millis(exact).map(|expiry| (expiry, exact)))
        .map(|(expiry, exact)| {
            let exact = format!("Expires {exact}");
            html! {
                <span class="quota-reset-credit-expiry" title={exact.clone()}>
                    {"expires "}
                    <Countdown
                        {expiry}
                        class={classes!("dash-overview-quota-age")}
                        title={exact}
                    />
                </span>
            }
        });
    Some(html! {
        <span class="quota-reset-credit" title={text.clone()}>
            <span class="quota-reset-credit-label">{text}</span>
            {expiry}
        </span>
    })
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
            {for credits.credits.iter().filter_map(render_reset_credit)}
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
                <ul class="quota-simple-list">{for accounts.iter().map(|entry| {
                    render_simple_row(&entry.provider, &entry.account, &entry.credential_type)
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
                <ul class="quota-simple-list">{for credentials.iter().map(|entry| {
                    render_simple_row(&entry.provider, &entry.account, &entry.credential_type)
                })}</ul>
            }
        </section>
    }
}

/// One single-line row; the title spells what the line may ellipsize.
fn render_simple_row(provider: &str, account: &wire::QuotaAccount, credential: &str) -> Html {
    let label = crate::quota::account_label(account);
    html! {
        <li title={format!("{provider} · {label} · {credential}")}>
            <strong>{provider.to_owned()}</strong>
            <span>{label}</span>
            <span>{credential.to_owned()}</span>
        </li>
    }
}
