//! Model providers on the Settings page: which ones pi can use, logging in
//! (pi's `pi auth login`, run in a pseudo-terminal and shown here) and out.
//! `/login` and `/logout` in a chat open this section.

use futures::StreamExt as _;
use gpui::{
    AnyElement, Context, ElementId, Entity, Focusable, FontWeight, SharedString, Subscription,
    Task, Window, div, px,
};

use super::{HubView, InstallationState};
use crate::pi_auth::{self, AuthState, LoginEvent, LoginProcess, ProviderStatus, Transcript};
use crate::text_input::TextInput;
use crate::ui::prelude::*;

pub(super) const LOGIN_CONTEXT: &str = "ProviderLogin";
/// Providers shown before the filter narrows them down.
const SHOWN_PROVIDERS: usize = 24;

pub(super) struct Providers {
    /// `None` while pi lists them.
    list: Option<Result<Vec<ProviderStatus>, String>>,
    filter: Entity<TextInput>,
    login: Option<Login>,
    /// A logout in flight.
    logging_out: Option<String>,
    /// The last logout or login result, shown under the list.
    message: Option<(String, Color)>,
    _list_task: Task<()>,
    _logout_task: Task<()>,
    _filter_subscription: Subscription,
}

struct Login {
    provider: String,
    api_key: bool,
    /// `None` once pi exited.
    process: Option<LoginProcess>,
    transcript: Transcript,
    input: Entity<TextInput>,
    /// The question the field answers; the field is focused when it changes.
    question: Option<String>,
    secret: bool,
    exit: Option<Option<i32>>,
    cancelled: bool,
    _task: Task<()>,
}

impl HubView {
    pub(super) fn ensure_providers(&mut self, cx: &mut Context<Self>) {
        if self.providers.is_some() {
            return;
        }
        let filter = cx.new(|cx| TextInput::single_line("Find a provider…", cx));
        let subscription = cx.observe(&filter, |_, _, cx| cx.notify());
        self.providers = Some(Providers {
            list: None,
            filter,
            login: None,
            logging_out: None,
            message: None,
            _list_task: Task::ready(()),
            _logout_task: Task::ready(()),
            _filter_subscription: subscription,
        });
        self.refresh_providers(cx);
    }

    fn refresh_providers(&mut self, cx: &mut Context<Self>) {
        let InstallationState::Ready(installation) = &self.installation else {
            if let Some(providers) = &mut self.providers {
                providers.list = Some(Err("pi is not ready yet.".to_string()));
            }
            return;
        };
        let installation = installation.clone();
        let Some(providers) = &mut self.providers else {
            return;
        };
        let task = cx.background_spawn(async move { pi_auth::providers(&installation) });
        providers._list_task = cx.spawn(async move |this, cx| {
            let list = task.await;
            this.update(cx, |this, cx| {
                if let Some(providers) = &mut this.providers {
                    providers.list = Some(list);
                    cx.notify();
                }
            })
            .ok();
        });
        cx.notify();
    }

    /// `/login [provider]` and `/logout`: the Settings page at the providers.
    pub(super) fn show_providers(
        &mut self,
        provider: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_settings(&super::OpenSettings, window, cx);
        self.ensure_providers(cx);
        match provider {
            Some(provider) if !provider.trim().is_empty() => {
                self.start_login(provider.trim().to_string(), false, window, cx)
            }
            _ => {
                if let Some(providers) = &self.providers {
                    window.focus(&providers.filter.read(cx).focus_handle(cx));
                }
            }
        }
    }

    pub(super) fn start_login(
        &mut self,
        provider: String,
        api_key: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let InstallationState::Ready(installation) = &self.installation else {
            return;
        };
        let installation = installation.clone();
        self.ensure_providers(cx);
        let Some(providers) = &mut self.providers else {
            return;
        };
        providers.message = None;
        let input = cx.new(|cx| TextInput::single_line("Your answer", cx));
        let mut login = Login {
            provider: provider.clone(),
            api_key,
            process: None,
            transcript: Transcript::default(),
            input,
            question: None,
            secret: false,
            exit: None,
            cancelled: false,
            _task: Task::ready(()),
        };
        match LoginProcess::start(&installation, &provider, api_key) {
            Ok((process, mut events)) => {
                login.process = Some(process);
                login._task = cx.spawn_in(window, async move |this, cx| {
                    while let Some(event) = events.next().await {
                        let alive = this
                            .update_in(cx, |this, window, cx| {
                                this.on_login_event(event, window, cx)
                            })
                            .is_ok();
                        if !alive {
                            return;
                        }
                    }
                });
            }
            Err(error) => {
                login
                    .transcript
                    .lines
                    .push(format!("Could not start pi: {error}"));
                login.exit = Some(None);
            }
        }
        providers.login = Some(login);
        cx.notify();
    }

    fn on_login_event(&mut self, event: LoginEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(login) = self
            .providers
            .as_mut()
            .and_then(|providers| providers.login.as_mut())
        else {
            return;
        };
        match event {
            LoginEvent::Output(bytes) => {
                login.transcript.feed(&bytes);
                let question = login.transcript.question().map(str::to_string);
                if question != login.question {
                    login.secret = question.as_deref().is_some_and(pi_auth::is_secret_question);
                    let secret = login.secret;
                    login.input.update(cx, |input, cx| {
                        input.clear(cx);
                        input.set_masked(secret, cx);
                    });
                    if question.is_some() {
                        window.focus(&login.input.read(cx).focus_handle(cx));
                    }
                    login.question = question;
                }
            }
            LoginEvent::Exited(code) => {
                login.exit = Some(code);
                login.process = None;
                login.question = None;
                if !login.transcript.current.trim().is_empty() {
                    let line = std::mem::take(&mut login.transcript.current);
                    login.transcript.lines.push(line);
                }
                let provider = login.provider.clone();
                if code == Some(0) {
                    self.login_finished(&provider, cx);
                }
            }
        }
        cx.notify();
    }

    /// Credentials changed: list again, and restart idle sessions so their
    /// pi reads them (a running pi keeps the credentials it started with).
    fn login_finished(&mut self, provider: &str, cx: &mut Context<Self>) {
        let chats = self.open_chats();
        let running = chats
            .iter()
            .filter(|chat| chat.read(cx).is_running())
            .count();
        let message = if chats.is_empty() {
            format!("pi can use {provider} now.")
        } else if running == 0 {
            self.restart_sessions(cx);
            format!("pi can use {provider} now. Open sessions were reloaded.")
        } else {
            crate::keys::in_text(&format!(
                "pi can use {provider} now. Reload sessions (⌘⇧R) when the work is done, so they use it too."
            ))
        };
        if let Some(providers) = &mut self.providers {
            providers.message = Some((message, Color::Success));
        }
        self.refresh_providers(cx);
    }

    fn submit_login_answer(&mut self, cx: &mut Context<Self>) {
        let Some(login) = self
            .providers
            .as_mut()
            .and_then(|providers| providers.login.as_mut())
        else {
            return;
        };
        let Some(process) = &mut login.process else {
            return;
        };
        if login.question.is_none() {
            return;
        }
        let answer = login.input.read(cx).text().to_string();
        if let Err(error) = process.answer(&answer) {
            login
                .transcript
                .lines
                .push(format!("Could not answer pi: {error}"));
        } else {
            login.transcript.answered(&answer, login.secret);
        }
        login.question = None;
        login.input.update(cx, |input, cx| input.clear(cx));
        cx.notify();
    }

    fn cancel_login(&mut self, cx: &mut Context<Self>) {
        let Some(providers) = &mut self.providers else {
            return;
        };
        match providers.login.as_mut() {
            Some(Login {
                process: Some(process),
                cancelled,
                ..
            }) => {
                process.cancel();
                *cancelled = true;
            }
            _ => providers.login = None,
        }
        cx.notify();
    }

    fn logout(&mut self, provider: String, cx: &mut Context<Self>) {
        let InstallationState::Ready(installation) = &self.installation else {
            return;
        };
        let installation = installation.clone();
        let Some(providers) = &mut self.providers else {
            return;
        };
        providers.logging_out = Some(provider.clone());
        providers.message = None;
        if providers
            .login
            .as_ref()
            .is_some_and(|login| login.process.is_none())
        {
            providers.login = None;
        }
        let task = cx.background_spawn({
            let provider = provider.clone();
            async move { pi_auth::logout(&installation, &provider) }
        });
        providers._logout_task = cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                if let Some(providers) = &mut this.providers {
                    providers.logging_out = None;
                    providers.message = Some(match result {
                        Ok(()) => (
                            crate::keys::in_text(&format!(
                                "Logged out of {provider}. Open sessions keep using it until they reload (⌘⇧R)."
                            )),
                            Color::Muted,
                        ),
                        Err(error) => (error, Color::Error),
                    });
                }
                this.refresh_providers(cx);
            })
            .ok();
        });
        cx.notify();
    }

    /// The "Model providers" part of the Settings page.
    pub(super) fn render_providers(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let Some(providers) = &self.providers else {
            return Vec::new();
        };
        let colors = cx.theme().colors.clone();
        let mut rows = Vec::new();
        rows.push(
            div()
                .pb(px(10.))
                .child(
                    Label::new(
                        "Log pi in to a provider with an account or an API key, like /login in pi's terminal. pi does the sign-in and keeps the credentials in ~/.pi/agent/auth.json; the app never reads them.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
                .into_any_element(),
        );

        if let Some(login) = &providers.login {
            rows.push(self.render_login(login, cx));
        }

        let list = match &providers.list {
            None => {
                rows.push(
                    h_flex()
                        .gap(px(6.))
                        .py(px(8.))
                        .child(Spinner::new().size(IconSize::XSmall))
                        .child(
                            LoadingLabel::new("Asking pi about providers").size(LabelSize::Small),
                        )
                        .into_any_element(),
                );
                return rows;
            }
            Some(Err(error)) => {
                rows.push(
                    Label::new(format!("Could not list providers: {error}"))
                        .size(LabelSize::Small)
                        .color(Color::Error)
                        .into_any_element(),
                );
                return rows;
            }
            Some(Ok(list)) => list,
        };

        // Providers pi can use, with a way out for stored credentials.
        let ready: Vec<&ProviderStatus> = list
            .iter()
            .filter(|provider| matches!(provider.state, AuthState::Ready(_)))
            .collect();
        let busy = providers
            .login
            .as_ref()
            .is_some_and(|login| login.process.is_some());
        let ready_rows = ready.iter().map(|provider| {
            let how = match &provider.state {
                AuthState::Ready(kind) if kind == "oauth" => "account",
                AuthState::Ready(kind) if kind == "api_key" && provider.stored => "API key",
                _ if provider.stored => "stored credentials",
                _ => "environment or models.json",
            };
            let id = provider.id.clone();
            let logging_out = providers.logging_out.as_deref() == Some(provider.id.as_str());
            h_flex()
                .gap(px(10.))
                .py(px(6.))
                .border_b_1()
                .border_color(colors.border_variant)
                .child(Indicator::dot(Color::Success))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .child(Label::new(provider.id.clone()).size(LabelSize::Small)),
                )
                .child(
                    Label::new(how)
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                )
                .when(provider.stored, |this| {
                    this.child(if logging_out {
                        Spinner::new().size(IconSize::XSmall).into_any_element()
                    } else {
                        Button::new(ElementId::Name(format!("logout:{id}").into()), "Log out")
                            .disabled(busy)
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.logout(id.clone(), cx)),
                            )
                            .into_any_element()
                    })
                })
                .into_any_element()
        });
        rows.push(
            div()
                .pt(px(4.))
                .pb(px(2.))
                .child(
                    Label::new(if ready.is_empty() {
                        "pi has no provider set up yet."
                    } else {
                        "Ready"
                    })
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .weight(FontWeight::MEDIUM),
                )
                .into_any_element(),
        );
        rows.extend(ready_rows);

        // Logging in to another provider.
        let filter = providers.filter.read(cx).text().trim().to_lowercase();
        let others: Vec<&ProviderStatus> = list
            .iter()
            .filter(|provider| !matches!(provider.state, AuthState::Ready(_)))
            .filter(|provider| filter.is_empty() || provider.id.contains(&filter))
            .collect();
        let shown = if filter.is_empty() {
            &others[..others.len().min(SHOWN_PROVIDERS)]
        } else {
            &others[..]
        };
        let chips = shown.iter().map(|provider| {
            let id = provider.id.clone();
            div()
                .id(ElementId::Name(format!("login:{id}").into()))
                .h(px(24.))
                .px(px(9.))
                .flex()
                .items_center()
                .rounded(px(5.))
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.element_background)
                .text_size(px(12.))
                .text_color(colors.text)
                .when(!busy, |this| {
                    this.cursor_pointer()
                        .hover(|style| style.bg(colors.element_hover))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.start_login(id.clone(), false, window, cx)
                        }))
                })
                .when(busy, |this| this.opacity(0.5))
                .child(provider.id.clone())
        });
        rows.push(
            h_flex()
                .gap(px(12.))
                .pt(px(18.))
                .pb(px(8.))
                .child(
                    div().flex_1().child(
                        Label::new("Log in to")
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .weight(FontWeight::MEDIUM),
                    ),
                )
                .child(
                    div()
                        .w(px(220.))
                        .rounded(px(6.))
                        .border_1()
                        .border_color(colors.border)
                        .bg(colors.editor_background)
                        .child(providers.filter.clone()),
                )
                .into_any_element(),
        );
        rows.push(
            h_flex()
                .flex_wrap()
                .gap(px(6.))
                .children(chips)
                .when(shown.len() < others.len(), |this| {
                    this.child(
                        Label::new(format!(
                            "and {} more: type to find them",
                            others.len() - shown.len()
                        ))
                        .size(LabelSize::XSmall)
                        .color(Color::Placeholder),
                    )
                })
                .when(others.is_empty() && !filter.is_empty(), |this| {
                    this.child(
                        Label::new("No provider matches. pi lists the ones it can log in to.")
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder),
                    )
                })
                .into_any_element(),
        );
        if let Some((message, color)) = &providers.message {
            rows.push(
                div()
                    .pt(px(10.))
                    .child(
                        Label::new(message.clone())
                            .size(LabelSize::Small)
                            .color(*color),
                    )
                    .into_any_element(),
            );
        }
        rows
    }

    fn render_login(&self, login: &Login, cx: &mut Context<Self>) -> AnyElement {
        let colors = cx.theme().colors.clone();
        let running = login.process.is_some();
        let (status, color): (SharedString, Color) = match login.exit {
            None => (
                format!("Logging in to {}", login.provider).into(),
                Color::Default,
            ),
            Some(Some(0)) => (
                format!("Logged in to {}", login.provider).into(),
                Color::Success,
            ),
            Some(_) if login.cancelled => (
                format!("Login to {} cancelled", login.provider).into(),
                Color::Muted,
            ),
            Some(_) => (
                format!("Login to {} did not finish", login.provider).into(),
                Color::Error,
            ),
        };
        let lines = login
            .transcript
            .lines
            .iter()
            .chain(
                (login.question.is_none() && !login.transcript.current.trim().is_empty())
                    .then_some(&login.transcript.current),
            )
            .filter(|line| !line.trim().is_empty())
            .enumerate()
            .map(|(index, line)| {
                let text = line.trim();
                if text.starts_with("https://") || text.starts_with("http://") {
                    let url = text.to_string();
                    div()
                        .id(ElementId::Name(format!("login-url:{index}").into()))
                        .cursor_pointer()
                        .text_color(colors.text_accent)
                        .tooltip(Tooltip::text("Open in the browser"))
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .child(
                            Label::new(text.to_string())
                                .size(LabelSize::Small)
                                .truncate(),
                        )
                        .into_any_element()
                } else {
                    Label::new(text.to_string())
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .into_any_element()
                }
            })
            .collect::<Vec<_>>();
        let provider = login.provider.clone();
        let api_key = login.api_key;
        let field = login.question.as_ref().map(|question| {
            v_flex()
                .gap(px(6.))
                .pt(px(6.))
                .child(Label::new(question.clone()).size(LabelSize::Small))
                .child(
                    h_flex()
                        .gap(px(6.))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .key_context(LOGIN_CONTEXT)
                                .rounded(px(6.))
                                .border_1()
                                .border_color(colors.border_focused)
                                .bg(colors.editor_background)
                                .on_action(cx.listener(|this, _: &crate::chat::Submit, _, cx| {
                                    this.submit_login_answer(cx)
                                }))
                                .child(login.input.clone()),
                        )
                        .child(
                            Button::new("login-answer", "Send")
                                .style(ButtonStyle::Filled)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.submit_login_answer(cx)),
                                ),
                        ),
                )
        });
        v_flex()
            .gap(px(4.))
            .mb(px(10.))
            .p(px(12.))
            .rounded(px(8.))
            .border_1()
            .border_color(colors.border)
            .bg(colors.elevated_surface_background)
            .child(
                h_flex()
                    .gap(px(8.))
                    .pb(px(4.))
                    .when(running, |this| {
                        this.child(Spinner::new().size(IconSize::XSmall))
                    })
                    .child(
                        div().flex_1().child(
                            Label::new(status)
                                .size(LabelSize::Small)
                                .weight(FontWeight::MEDIUM)
                                .color(color),
                        ),
                    )
                    .when(running && !api_key && !login.secret, |this| {
                        this.child(
                            Button::new("login-api-key", "Use an API key instead").on_click(
                                cx.listener(move |this, _, window, cx| {
                                    if let Some(process) = this
                                        .providers
                                        .as_mut()
                                        .and_then(|providers| providers.login.as_mut())
                                        .and_then(|login| login.process.as_mut())
                                    {
                                        process.cancel();
                                    }
                                    this.start_login(provider.clone(), true, window, cx)
                                }),
                            ),
                        )
                    })
                    .child(
                        Button::new("login-close", if running { "Cancel" } else { "Close" })
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_login(cx))),
                    ),
            )
            .children(lines)
            .children(field)
            .into_any_element()
    }
}
