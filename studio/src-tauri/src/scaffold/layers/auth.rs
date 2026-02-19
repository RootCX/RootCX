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
    format!(r##"import {{ useState }} from "react";
import {{ useAuth }} from "@rootcx/runtime";
import {{ Card, CardHeader, CardTitle, CardDescription, CardContent }} from "@/components/ui/card";
import {{ Button }} from "@/components/ui/button";
import {{ Input }} from "@/components/ui/input";
import {{ Label }} from "@/components/ui/label";

export default function App() {{
  const {{ user, loading, isAuthenticated, login, register, logout }} = useAuth();
  const [mode, setMode] = useState<"login" | "register">("login");
  const [error, setError] = useState<string | null>(null);

  if (loading) {{
    return (
      <div className="flex min-h-screen items-center justify-center">
        <p className="text-muted-foreground">Loading…</p>
      </div>
    );
  }}

  if (!isAuthenticated) {{
    return (
      <div className="flex min-h-screen items-center justify-center bg-background px-4">
        <Card className="w-full max-w-sm">
          <CardHeader>
            <CardTitle className="text-2xl">{name}</CardTitle>
            <CardDescription>
              {{mode === "login" ? "Sign in to your account" : "Create a new account"}}
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form
              className="space-y-4"
              onSubmit={{async (e) => {{
                e.preventDefault();
                setError(null);
                const fd = new FormData(e.currentTarget);
                const username = fd.get("username") as string;
                const password = fd.get("password") as string;
                try {{
                  if (mode === "register") {{
                    await register({{ username, password }});
                    await login(username, password);
                  }} else {{
                    await login(username, password);
                  }}
                }} catch (err) {{
                  setError(err instanceof Error ? err.message : "Authentication failed");
                }}
              }}}}
            >
              {{error && <p className="text-sm text-destructive">{{error}}</p>}}
              <div className="space-y-2">
                <Label htmlFor="username">Username</Label>
                <Input id="username" name="username" placeholder="Username" required />
              </div>
              <div className="space-y-2">
                <Label htmlFor="password">Password</Label>
                <Input id="password" name="password" type="password" placeholder="Password" minLength={{8}} required />
              </div>
              <Button type="submit" className="w-full">
                {{mode === "login" ? "Sign in" : "Create account"}}
              </Button>
              <p className="text-center text-sm text-muted-foreground">
                {{mode === "login" ? "No account? " : "Already have one? "}}
                <button
                  type="button"
                  className="text-primary underline-offset-4 hover:underline"
                  onClick={{() => {{ setMode(mode === "login" ? "register" : "login"); setError(null); }}}}
                >
                  {{mode === "login" ? "Register" : "Sign in"}}
                </button>
              </p>
            </form>
          </CardContent>
        </Card>
      </div>
    );
  }}

  return (
    <div className="flex min-h-screen items-center justify-center bg-background px-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="text-2xl">{name}</CardTitle>
          <CardDescription>Signed in as {{user!.username}}</CardDescription>
        </CardHeader>
        <CardContent>
          <Button variant="outline" className="w-full" onClick={{() => logout()}}>Sign out</Button>
        </CardContent>
      </Card>
    </div>
  );
}}
"##)
}
