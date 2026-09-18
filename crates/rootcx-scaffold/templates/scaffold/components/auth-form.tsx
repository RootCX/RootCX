import type { AuthFormSlotProps } from "@rootcx/sdk";
import {
  Alert, AlertDescription, Button,
  Card, CardHeader, CardTitle, CardDescription, CardContent, CardFooter,
  Field, FieldGroup, FieldLabel, FieldDescription, Input, Separator, Spinner,
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
  mode, setMode, error, submitting, onSubmit, appTitle,
  providers, onOidcLogin, passwordLoginEnabled,
}: AuthFormSlotProps) {
  const registering = mode === "register";

  return (
    <div className="flex min-h-svh items-center justify-center p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle><h1>{appTitle}</h1></CardTitle>
          <CardDescription>
            {registering ? "Create a new account" : "Sign in to your account"}
          </CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {providers.map((provider) => (
            <Button key={provider.id} type="button" variant="outline"
              disabled={submitting} onClick={() => onOidcLogin(provider.id)}>
              Sign in with {provider.displayName}
            </Button>
          ))}
          {providers.length > 0 && passwordLoginEnabled && <Separator />}
          {error && (
            <Alert variant="destructive">
              <AlertDescription>{error}</AlertDescription>
            </Alert>
          )}
          {passwordLoginEnabled && (
            <form onSubmit={onSubmit}>
              <FieldGroup>
                <Field data-disabled={submitting}>
                  <FieldLabel htmlFor="email">Email</FieldLabel>
                  <Input id="email" name="email" type="email" autoComplete="email"
                    required disabled={submitting} />
                </Field>
                <Field data-disabled={submitting}>
                  <FieldLabel htmlFor="password">Password</FieldLabel>
                  <Input id="password" name="password" type="password"
                    autoComplete={registering ? "new-password" : "current-password"}
                    minLength={6} required disabled={submitting}
                    aria-describedby={registering ? "password-help" : undefined} />
                  {registering && (
                    <FieldDescription id="password-help">Must be at least 6 characters.</FieldDescription>
                  )}
                </Field>
                {registering && (
                  <Field data-disabled={submitting}>
                    <FieldLabel htmlFor="confirmPassword">Confirm password</FieldLabel>
                    <Input id="confirmPassword" name="confirmPassword" type="password"
                      autoComplete="new-password" minLength={6} required disabled={submitting} />
                  </Field>
                )}
                <Button type="submit" disabled={submitting} aria-busy={submitting}>
                  {submitting && <Spinner data-icon="inline-start" />}
                  {registering ? "Create account" : "Sign in"}
                </Button>
              </FieldGroup>
            </form>
          )}
          {!passwordLoginEnabled && providers.length === 0 && (
            <Alert><AlertDescription>No login methods available.</AlertDescription></Alert>
          )}
        </CardContent>
        {passwordLoginEnabled && (
          <CardFooter className="justify-center gap-2">
            <span className="text-sm text-muted-foreground">
              {registering ? "Already have an account?" : "No account?"}
            </span>
            <Button type="button" variant="link" disabled={submitting}
              onClick={() => setMode(registering ? "login" : "register")}>
              {registering ? "Sign in" : "Register"}
            </Button>
          </CardFooter>
        )}
      </Card>
    </div>
  );
}
