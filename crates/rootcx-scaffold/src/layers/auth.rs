use crate::emitter::Emitter;
use crate::types::{Layer, LayerFuture, ScaffoldContext};

pub(super) const TPL_AUTH_FORM: &str =
    include_str!("../../templates/scaffold/components/auth-form.tsx");

pub struct AuthLayer {
    pub include_auth: bool,
}

impl Layer for AuthLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let content = if self.include_auth { auth_app(&ctx.app_id) } else { simple_app(&ctx.app_id) };
            if self.include_auth {
                e.write("src/components/auth-form.tsx", TPL_AUTH_FORM).await?;
            }
            e.write("src/App.tsx", &content).await
        })
    }
}

fn simple_app(name: &str) -> String {
    include_str!("../../templates/scaffold/simple-app.tsx").replace("__APP_ID__", name)
}

fn auth_app(name: &str) -> String {
    include_str!("../../templates/scaffold/auth-app.tsx").replace("__APP_ID__", name)
}
