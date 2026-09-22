import type { AuthFormSlotProps } from "@rootcx/sdk";
import {
  Alert, AlertDescription, Button,
  Card, CardHeader, CardTitle, CardDescription, CardContent,
  Spinner,
} from "@rootcx/ui";

export function AuthLoading() {
  return (
    <div className="flex min-h-svh items-center justify-center gap-2" role="status">
      <Spinner />
      <span>Loading…</span>
    </div>
  );
}

export function AuthForm({
  error, submitting, appTitle, providers, onOidcLogin,
}: AuthFormSlotProps) {
  return (
    <div className="flex min-h-svh items-center justify-center p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle><h1>{appTitle}</h1></CardTitle>
          <CardDescription>Sign in to your workspace</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {error && <Alert variant="destructive"><AlertDescription>{error}</AlertDescription></Alert>}
          {providers.map(provider => (
            <Button key={provider.id} type="button" disabled={submitting}
              onClick={() => onOidcLogin(provider.id)}>
              Continue with {provider.displayName}
            </Button>
          ))}
          {providers.length === 0 && (
            <Alert><AlertDescription>No sign-in provider is configured. Contact your workspace administrator.</AlertDescription></Alert>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
