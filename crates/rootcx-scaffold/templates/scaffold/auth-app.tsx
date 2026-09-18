import { NavLink, Navigate, useLocation, Routes, Route } from "react-router-dom";
import { AuthGate } from "@rootcx/sdk";
import { AuthForm, AuthLoading } from "@/components/auth-form";
import {
  Button,
  Page, PageHeader, PageHeading, PageTitle, PageDescription, PageTopbar,
  Sidebar, SidebarProvider, SidebarHeader, SidebarContent, SidebarFooter,
  SidebarGroup, SidebarMenu, SidebarMenuItem, SidebarMenuButton,
  SidebarInset, SidebarTrigger,
} from "@rootcx/ui";
import { IconLogout, IconHome } from "@tabler/icons-react";

export default function App() {
  const { pathname } = useLocation();

  return (
    <AuthGate appTitle="__APP_ID__" renderForm={(props) => <AuthForm {...props} />} renderLoading={AuthLoading}>
      {({ user, logout }) => (
        <SidebarProvider>
          <Sidebar>
            <SidebarHeader>
              <span className="truncate text-sm font-semibold">__APP_ID__</span>
            </SidebarHeader>
            <SidebarContent>
              <SidebarGroup>
                <SidebarMenu>
                  <SidebarMenuItem>
                    <SidebarMenuButton asChild isActive={pathname === "/"}>
                      <NavLink to="/"><IconHome /><span>Home</span></NavLink>
                    </SidebarMenuButton>
                  </SidebarMenuItem>
                </SidebarMenu>
              </SidebarGroup>
            </SidebarContent>
            <SidebarFooter>
              <div className="flex items-center justify-between gap-2">
                <span className="truncate text-sm text-muted-foreground">{user.email}</span>
                <Button variant="ghost" size="icon" onClick={() => logout()} aria-label="Sign out">
                  <IconLogout />
                </Button>
              </div>
            </SidebarFooter>
          </Sidebar>
          <SidebarInset>
            <PageTopbar>
              <SidebarTrigger />
              <span className="text-sm font-semibold">__APP_ID__</span>
            </PageTopbar>
            <Routes>
              <Route path="/" element={
                <Page>
                  <PageHeader>
                    <PageHeading>
                      <PageTitle>Home</PageTitle>
                      <PageDescription>Welcome to __APP_ID__</PageDescription>
                    </PageHeading>
                  </PageHeader>
                </Page>
              } />
              <Route path="*" element={<Navigate to="/" replace />} />
            </Routes>
          </SidebarInset>
        </SidebarProvider>
      )}
    </AuthGate>
  );
}
