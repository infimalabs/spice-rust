use super::*;
use crate::browser::clock::Age;

pub(super) fn render_info(runtime: Option<&CanvasRuntime>) -> Html {
    let Some(generation) = runtime.and_then(CanvasRuntime::quota) else {
        return section_unavailable();
    };
    let presentation = crate::quota::present(generation);
    let snapshot = generation.last_good.as_deref();
    html! {
        <section
            class="section info-quota-section"
            aria-labelledby="provider-quota-heading"
            data-quota-state={presentation.condition.key()}
        >
            <div class="section-heading">
                <div>
                    <h2 id="provider-quota-heading">{"Provider quota"}</h2>
                    <p class="info-section-copy">{"Redacted account usage sampled by the resident; quota never controls allocation or routing."}</p>
                </div>
                <span class="section-count">{format!("Generation {}", generation.generation)}</span>
            </div>
            <div class="quota-status" role="status" aria-live="polite">
                <strong>{presentation.condition.label()}</strong>
                <span>{presentation.detail}</span>
                if let Some(sampled_at) = presentation.sampled_at_ms {
                    <span>{"Sample age "}<Age authored={sampled_at} class={classes!("quota-info-age")} title="Age of retained successful quota data" /></span>
                }
            </div>
            {render_attempt(generation.latest_attempt.as_ref())}
            if let Some(snapshot) = snapshot {
                {render_capacity(generation)}
                {render_reports(&snapshot.reports)}
                {render_accounts_without_usage(&snapshot.accounts_without_usage)}
                {render_disabled(&snapshot.disabled_credentials)}
            }
        </section>
    }
}

fn section_unavailable() -> Html {
    html! {
        <section class="section info-quota-section" aria-labelledby="provider-quota-heading" data-quota-state="unavailable">
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
            <Age authored={i64::try_from(*finished_at_ms).unwrap_or(i64::MAX)} class={classes!("quota-info-age")} title="Age of latest failed quota attempt" />
            {" ago"}
        </p>
    }
}

fn render_capacity(generation: &wire::QuotaGeneration) -> Html {
    let snapshot = generation
        .last_good
        .as_deref()
        .expect("capacity renders only with a retained quota snapshot");
    html! {
        <section class="quota-detail-group" aria-labelledby="quota-capacity-heading">
            <h3 id="quota-capacity-heading">{"Remaining provider quota"}</h3>
            if snapshot.reports.is_empty() {
                <p class="quota-empty">{"No account usage reports were returned."}</p>
            } else {
                <dl class="quota-capacity-grid">
                    <div>
                        <dt>{"Total across all account capacity"}</dt>
                        <dd><strong>{crate::quota::capacity_summary(generation)}</strong></dd>
                        <dd>{"Plan-weighted estimate · Free 0.2× · $20 plans 1× · Pro/Max 5× or explicit 20×"}</dd>
                    </div>
                </dl>
            }
        </section>
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
    html! {
        <article class="quota-account" aria-labelledby={heading.clone()}>
            <header>
                <div>
                    <span class="quota-provider">{report.provider.clone()}</span>
                    <h4 id={heading}>{crate::quota::account_label(&report.account)}</h4>
                </div>
                <Age authored={report.fetched_at_ms} class={classes!("quota-info-age")} title="Age of this provider report" />
            </header>
            {render_account_metadata(&report.account)}
            <div class="quota-limit-list">
                {for report.limits.iter().map(render_limit)}
            </div>
            {render_reset_credits(report.reset_credits.as_ref())}
        </article>
    }
}

fn render_account_metadata(account: &wire::QuotaAccount) -> Html {
    let mut rows = Vec::new();
    push_optional(&mut rows, "Email", account.email.as_deref());
    push_optional(&mut rows, "Account", account.account_id.as_deref());
    push_optional(&mut rows, "Project", account.project_id.as_deref());
    push_optional(&mut rows, "Organization", account.org_id.as_deref());
    push_optional(&mut rows, "Organization name", account.org_name.as_deref());
    push_optional(&mut rows, "Plan", account.plan_type.as_deref());
    html! { <dl class="quota-account-metadata">{for rows}</dl> }
}

fn push_optional(rows: &mut Vec<Html>, label: &'static str, value: Option<&str>) {
    if let Some(value) = value {
        rows.push(html! { <div><dt>{label}</dt><dd>{value.to_owned()}</dd></div> });
    }
}

fn render_limit(limit: &wire::QuotaLimit) -> Html {
    let used = limit.amount.used.as_deref().unwrap_or("unknown");
    let remaining = limit.amount.remaining.as_deref().unwrap_or("unknown");
    let maximum = limit.amount.limit.as_deref().unwrap_or("unknown");
    html! {
        <section class="quota-limit">
            <header><strong>{limit.label.clone()}</strong><span>{limit.status.clone().unwrap_or_else(|| "reported".to_owned())}</span></header>
            <dl>
                <div><dt>{"Used"}</dt><dd>{format!("{used} {}", limit.amount.unit)}</dd></div>
                <div><dt>{"Remaining"}</dt><dd>{format!("{remaining} {}", limit.amount.unit)}</dd></div>
                <div><dt>{"Limit"}</dt><dd>{format!("{maximum} {}", limit.amount.unit)}</dd></div>
                if let Some(window) = &limit.window {
                    <div><dt>{"Reset"}</dt><dd>{render_reset(window)}</dd></div>
                }
            </dl>
        </section>
    }
}

fn render_reset(window: &wire::QuotaWindow) -> Html {
    match (window.reset_label.as_ref(), window.resets_at) {
        (Some(label), Some(at)) => {
            html! { <>{label.clone()}{" · "}<time data-epoch-millis={at.to_string()}>{iso_time(at)}</time></> }
        }
        (Some(label), None) => html! { {label.clone()} },
        (None, Some(at)) => {
            html! { <time data-epoch-millis={at.to_string()}>{iso_time(at)}</time> }
        }
        (None, None) => html! { {"Not reported"} },
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
        <div class="quota-reset-credits">
            <strong>{"Reset credits"}</strong>
            <span>{format!("{} available", credits.available_count)}</span>
            if let Some(redeemable) = credits.redeemable_count {
                <span>{format!("{redeemable} redeemable")}</span>
            }
            if !credits.credits.is_empty() {
                <ul>{for credits.credits.iter().map(|credit| html! {
                    <li>{credit.title.clone().or_else(|| credit.program.clone()).unwrap_or_else(|| "Reset credit".to_owned())}{credit.remaining_count.map(|count| format!(" · {count} remaining")).unwrap_or_default()}</li>
                })}</ul>
            }
        </div>
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
