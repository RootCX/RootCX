use crate::scaffold::emitter::Emitter;
use crate::scaffold::types::{Layer, LayerFuture, ScaffoldContext};

/// Emits: src/App.tsx with login/register flow using @rootcx/sdk auth.
/// When `include_auth` is false, emits a simple hello-world App instead.
pub struct AuthLayer {
    pub include_auth: bool,
}

impl Layer for AuthLayer {
    fn emit<'a>(&'a self, ctx: &'a ScaffoldContext, e: &'a Emitter) -> LayerFuture<'a> {
        Box::pin(async move {
            let content = if self.include_auth { auth_app(&ctx.app_id) } else { simple_app(&ctx.app_id) };
            e.write("src/App.tsx", &content).await
        })
    }
}

fn simple_app(name: &str) -> String {
    format!(
        r#"import {{ PageHeader }} from "@rootcx/ui";

export default function App() {{
  return (
    <div className="p-6">
      <PageHeader title="{name}" description="Get started by editing src/App.tsx" />
    </div>
  );
}}
"#
    )
}

fn auth_app(name: &str) -> String {
    format!(
        r#"import {{ AuthGate }} from "@rootcx/sdk";
import {{ AppShell, AppShellSidebar, AppShellMain, Sidebar, SidebarItem, PageHeader, Button }} from "@rootcx/ui";
import {{ IconLogout, IconHome }} from "@tabler/icons-react";

export default function App() {{
  return (
    <AuthGate appTitle="{name}">
      {{({{ user, logout }}) => (
        <AppShell>
          <AppShellSidebar>
            <Sidebar
              header={{<span className="text-sm font-semibold">{name}</span>}}
              footer={{
                <div className="flex items-center justify-between">
                  <span className="truncate text-sm text-muted-foreground">{{user.username}}</span>
                  <Button variant="ghost" size="icon" onClick={{() => logout()}}>
                    <IconLogout className="h-4 w-4" />
                  </Button>
                </div>
              }}
            >
              <SidebarItem icon={{<IconHome />}} label="Home" active />
            </Sidebar>
          </AppShellSidebar>
          <AppShellMain>
            <div className="p-6">
              <PageHeader title="Home" description="Welcome to {name}" />
            </div>
          </AppShellMain>
        </AppShell>
      )}}
    </AuthGate>
  );
}}
"#
    )
}
