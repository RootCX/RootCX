use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, ScaffoldContext};
use std::future::Future;
use std::pin::Pin;

/// Emits: src/App.tsx with login/register flow using @rootcx/runtime auth.
/// When `include_auth` is false, emits a simple hello-world App instead.
pub struct AuthLayer {
    pub include_auth: bool,
}

impl Layer for AuthLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
        let include_auth = self.include_auth;
        Box::pin(async move {
            let content = if include_auth {
                auth_app(&ctx.name)
            } else {
                simple_app(&ctx.name)
            };
            e.write("src/App.tsx", &content).await
        })
    }
}

fn simple_app(name: &str) -> String {
    format!(r#"export default function App() {{
  return (
    <div className="flex min-h-screen items-center justify-center bg-background">
      <h1 className="text-4xl font-bold">{name}</h1>
    </div>
  );
}}
"#)
}

fn auth_app(name: &str) -> String {
    format!(r#"import {{ AuthGate }} from "@rootcx/runtime";

export default function App() {{
  return (
    <AuthGate appTitle="{name}">
      {{({{ user, logout }}) => (
        <div className="flex min-h-screen items-center justify-center bg-background px-4">
          <div className="w-full max-w-sm rounded-lg border bg-card p-6 shadow-sm text-center space-y-4">
            <h2 className="text-2xl font-semibold tracking-tight">{name}</h2>
            <p className="text-sm text-muted-foreground">Signed in as {{user.username}}</p>
            <button
              className="inline-flex h-10 w-full items-center justify-center rounded-md border bg-background px-4 py-2 text-sm font-medium hover:bg-muted"
              onClick={{() => logout()}}
            >
              Sign out
            </button>
          </div>
        </div>
      )}}
    </AuthGate>
  );
}}
"#)
}
