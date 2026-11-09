//! The dashboard: server-rendered pages (askama), htmx for Load more and the
//! token, uPlot for the charts. All static files are in the binary.

use crate::app::{with_db, Shared};
use crate::query::Footer;
use crate::tz::now_ms;
use crate::views::{self, Ctx, Params};
use askama::Template;
use axum::extract::{Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::routing::{get, post};
use axum::{Form, Router};
use std::collections::HashMap;

type Q = Query<HashMap<String, String>>;

pub fn routes() -> Router<Shared> {
    Router::new()
        .route("/", get(overview))
        .route("/layouts", get(layouts))
        .route("/layout", get(layout))
        .route("/scripts", get(scripts))
        .route("/script", get(script))
        .route("/users", get(users))
        .route("/user", get(user))
        .route("/clients", get(clients))
        .route("/client", get(client))
        .route("/freezes", get(freezes))
        .route("/freezes/more", get(freezes_more))
        .route("/freeze", get(freeze))
        .route("/setup", get(setup).post(setup_save))
        .route("/setup/token", post(setup_token))
        .route("/static/{*path}", get(asset))
}

// ---------------------------------------------------------------- templates

#[derive(Template)]
#[template(path = "overview.html")]
struct OverviewT {
    shell: Shell,
    v: views::Overview,
}

#[derive(Template)]
#[template(path = "layouts.html")]
struct LayoutsT {
    shell: Shell,
    v: views::Layouts,
}

#[derive(Template)]
#[template(path = "layout.html")]
struct LayoutT {
    shell: Shell,
    v: views::LayoutDetail,
}

#[derive(Template)]
#[template(path = "scripts.html")]
struct ScriptsT {
    shell: Shell,
    v: views::Scripts,
}

#[derive(Template)]
#[template(path = "script.html")]
struct ScriptT {
    shell: Shell,
    v: views::ScriptDetail,
}

#[derive(Template)]
#[template(path = "users.html")]
struct UsersT {
    shell: Shell,
    v: views::Users,
}

#[derive(Template)]
#[template(path = "user.html")]
struct UserT {
    shell: Shell,
    v: views::UserDetail,
}

#[derive(Template)]
#[template(path = "clients.html")]
struct ClientsT {
    shell: Shell,
    v: views::Clients,
}

#[derive(Template)]
#[template(path = "client.html")]
struct ClientT {
    shell: Shell,
    v: views::ClientDetail,
}

#[derive(Template)]
#[template(path = "freezes.html")]
struct FreezesT {
    shell: Shell,
    v: views::Freezes,
}

#[derive(Template)]
#[template(path = "freeze_rows.html")]
struct FreezeRowsT {
    v: views::Freezes,
}

#[derive(Template)]
#[template(path = "freeze.html")]
struct FreezeT {
    shell: Shell,
    v: views::FreezeDetail,
}

#[derive(Template)]
#[template(path = "setup.html")]
struct SetupT {
    shell: Shell,
    v: views::Setup,
    error: String,
}

#[derive(Template)]
#[template(path = "token.html")]
struct TokenT {
    v: views::Setup,
}

fn html<T: Template>(t: T) -> Response {
    match t.render() {
        Ok(s) => Html(s).into_response(),
        Err(e) => {
            crate::logln!("template error: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, "template error").into_response()
        }
    }
}

/// Render a page on the blocking pool with a read context.
async fn page<F>(app: &Shared, q: HashMap<String, String>, f: F) -> Response
where
    F: FnOnce(&Ctx, &Params) -> Response + Send + 'static,
{
    with_db(app, move |a, db| {
        let now = now_ms();
        let ctx = Ctx::new(a, db, now);
        let p = Params::from_map(&q);
        f(&ctx, &p)
    })
    .await
}

/// The parts of every page: nav, title, footer.
#[derive(Debug, Clone)]
pub struct Shell {
    pub nav: &'static str,
    /// Cache-busting tag for the static links.
    pub asset: &'static str,
    pub title: String,
    pub footer: Footer,
}

fn sh(ctx: &Ctx, nav: &'static str, title: &str) -> Shell {
    Shell {
        nav,
        asset: asset_tag(),
        title: title.into(),
        footer: crate::query::footer(ctx.app, ctx.db, ctx.now),
    }
}

async fn overview(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        html(OverviewT {
            shell: sh(c, "overview", "Overview"),
            v: views::overview(c, p),
        })
    })
    .await
}

async fn layouts(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        html(LayoutsT {
            shell: sh(c, "layouts", "Layouts"),
            v: views::layouts(c, p),
        })
    })
    .await
}

async fn layout(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        let v = views::layout_detail(c, p);
        html(LayoutT {
            shell: sh(c, "layouts", &v.name),
            v,
        })
    })
    .await
}

async fn scripts(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        html(ScriptsT {
            shell: sh(c, "scripts", "Scripts"),
            v: views::scripts(c, p),
        })
    })
    .await
}

async fn script(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        let v = views::script_detail(c, p);
        html(ScriptT {
            shell: sh(c, "scripts", &v.name),
            v,
        })
    })
    .await
}

async fn users(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        html(UsersT {
            shell: sh(c, "users", "Users"),
            v: views::users(c, p),
        })
    })
    .await
}

async fn user(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        let v = views::user_detail(c, p);
        html(UserT {
            shell: sh(c, "users", &v.account),
            v,
        })
    })
    .await
}

async fn clients(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        html(ClientsT {
            shell: sh(c, "clients", "Clients"),
            v: views::clients(c, p),
        })
    })
    .await
}

async fn client(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        let v = views::client_detail(c, p);
        let status = if v.found {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        let mut r = html(ClientT {
            shell: sh(c, "clients", &v.name),
            v,
        });
        *r.status_mut() = status;
        r
    })
    .await
}

async fn freezes(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        html(FreezesT {
            shell: sh(c, "freezes", "Freezes"),
            v: views::freezes(c, p),
        })
    })
    .await
}

/// The next page of the Freezes table (htmx: rows plus a new Load more).
async fn freezes_more(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        html(FreezeRowsT {
            v: views::freezes(c, p),
        })
    })
    .await
}

async fn freeze(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        let v = views::freeze_detail(c, p);
        let status = if v.found {
            StatusCode::OK
        } else {
            StatusCode::NOT_FOUND
        };
        let mut r = html(FreezeT {
            shell: sh(c, "freezes", "Freeze"),
            v,
        });
        *r.status_mut() = status;
        r
    })
    .await
}

async fn setup(State(app): State<Shared>, Query(q): Q) -> Response {
    page(&app, q, |c, p| {
        html(SetupT {
            shell: sh(c, "setup", "Setup"),
            v: views::setup(c, p, false),
            error: String::new(),
        })
    })
    .await
}

async fn setup_save(
    State(app): State<Shared>,
    h: HeaderMap,
    Form(form): Form<HashMap<String, String>>,
) -> Response {
    if !crate::api::same_origin(&h) {
        return (StatusCode::FORBIDDEN, "cross-site request").into_response();
    }
    with_db(&app, move |a, db| {
        let now = now_ms();
        let old = a.settings(db);
        match views::parse_setup(&form, &old) {
            Ok(s) => match s.save(db) {
                Ok(()) => Redirect::to("/setup?saved=1").into_response(),
                Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()).into_response(),
            },
            Err(e) => {
                let ctx = Ctx::new(a, db, now);
                let p = Params::default();
                let mut r = html(SetupT {
                    shell: sh(&ctx, "setup", "Setup"),
                    v: views::setup(&ctx, &p, false),
                    error: e,
                });
                *r.status_mut() = StatusCode::BAD_REQUEST;
                r
            }
        }
    })
    .await
}

/// New Event token (htmx swaps the token block and the startup step).
async fn setup_token(State(app): State<Shared>, h: HeaderMap) -> Response {
    if !crate::api::same_origin(&h) {
        return (StatusCode::FORBIDDEN, "cross-site request").into_response();
    }
    let a = app.clone();
    if tokio::task::spawn_blocking(move || a.new_token())
        .await
        .ok()
        .and_then(|r| r.ok())
        .is_none()
    {
        return (StatusCode::INTERNAL_SERVER_ERROR, "cannot make a token").into_response();
    }
    let q: HashMap<String, String> = [("show".to_string(), "1".to_string())].into();
    page(&app, q, |c, p| {
        html(TokenT {
            v: views::setup(c, p, false),
        })
    })
    .await
}

// ---------------------------------------------------------------- static

const APP_CSS: &str = include_str!("../static/app.css");
const APP_JS: &str = include_str!("../static/app.js");
const HTMX: &str = include_str!("../static/vendor/htmx.min.js");
const UPLOT_JS: &str = include_str!("../static/vendor/uPlot.iife.min.js");
const UPLOT_CSS: &str = include_str!("../static/vendor/uPlot.min.css");
const ICON: &str = include_str!("../static/icon.svg");
const LICENSES: &str = concat!(
    "htmx 2.0.4 (https://htmx.org)\n\n",
    include_str!("../static/vendor/htmx-LICENSE.txt"),
    "\n\nuPlot 1.6.31 (https://github.com/leeoniya/uPlot)\n\n",
    include_str!("../static/vendor/uPlot-LICENSE.txt")
);

/// A short hash of the app files, for cache-busting links.
pub fn asset_tag() -> &'static str {
    static TAG: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    TAG.get_or_init(|| {
        let mut h: u64 = 0xcbf29ce484222325;
        for b in APP_CSS
            .bytes()
            .chain(APP_JS.bytes())
            .chain(crate::app::VERSION.bytes())
        {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        format!("{:08x}", h as u32)
    })
}

async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let (body, ty) = match path.as_str() {
        "app.css" => (APP_CSS, "text/css; charset=utf-8"),
        "app.js" => (APP_JS, "text/javascript; charset=utf-8"),
        "vendor/htmx.min.js" => (HTMX, "text/javascript; charset=utf-8"),
        "vendor/uPlot.iife.min.js" => (UPLOT_JS, "text/javascript; charset=utf-8"),
        "vendor/uPlot.min.css" => (UPLOT_CSS, "text/css; charset=utf-8"),
        "icon.svg" => (ICON, "image/svg+xml"),
        "licenses.txt" => (LICENSES, "text/plain; charset=utf-8"),
        _ => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    (
        [
            (header::CONTENT_TYPE, ty),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body,
    )
        .into_response()
}
