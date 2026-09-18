// Run with playwright-cli run-code; previews: simple/auth/agent/cli on ports 5891–5894.
// All Core requests are intercepted; no live accounts or records are changed.
async page => {
  const check = (condition, message) => { if (!condition) throw new Error(message); };
  const results = [];
  const pageErrors = [];
  page.on('pageerror', error => pageErrors.push(error.message));
  await page.addInitScript(() => localStorage.clear());
  let authenticated = false;
  let failLogin = false;
  let passwordLoginEnabled = true;
  let providers = [{ id: 'audit-sso', displayName: 'Audit SSO' }];
  let registrations = 0;
  let ssoUrl = '';
  const user = { id: 'audit-user', email: 'audit@example.test', displayName: 'Audit', createdAt: '2026-09-17T00:00:00Z' };
  await page.unrouteAll();
  await page.route('**/api/v1/**', async route => {
    const href = route.request().url();
    const pathname = '/' + href.split('?')[0].split('/').slice(3).join('/');
    const respond = (body, status = 200) => route.fulfill({ status, json: body });
    if (pathname === '/api/v1/auth/mode') return respond({ authRequired: true, setupRequired: false, passwordLoginEnabled, magicLinkEnabled: false, providers });
    if (pathname === '/api/v1/auth/me') return respond(authenticated ? user : { error: 'Unauthorized' }, authenticated ? 200 : 401);
    if (pathname === '/api/v1/auth/register') { registrations++; return respond(user); }
    if (pathname === '/api/v1/auth/login') {
      const body = route.request().postDataJSON();
      check(body.email === user.email && body.password === 'Audit-password-123', 'Auth form did not submit the expected fields');
      if (failLogin) return respond({ error: 'Invalid credentials' }, 401);
      authenticated = true;
      return respond({ user, accessToken: 'audit-access', refreshToken: 'audit-refresh', expiresIn: 3600 });
    }
    if (pathname === '/api/v1/auth/logout') { authenticated = false; return respond({}); }
    if (pathname.endsWith('/authorize')) {
      ssoUrl = href;
      return route.fulfill({ contentType: 'text/html', body: '<h1>Intercepted SSO navigation</h1>' });
    }
    await route.abort();
    throw new Error('Unexpected API call: ' + pathname);
  });
  async function revealSignOut() {
    await page.waitForFunction(() => document.querySelector('[data-slot=sidebar-trigger]') || document.querySelector('button[aria-label="Sign out"]'));
    if (!await page.getByRole('button', { name: 'Sign out', exact: true }).isVisible()) {
      await page.locator('[data-slot=sidebar-trigger]').click();
    }
    await page.getByRole('button', { name: 'Sign out', exact: true }).waitFor();
  }
  for (const [index, variant] of ['simple', 'auth', 'agent', 'cli'].entries()) {
    const url = `http://127.0.0.1:${5891 + index}`;
    authenticated = false;
    passwordLoginEnabled = true;
    providers = [{ id: 'audit-sso', displayName: 'Audit SSO' }];
    await page.setViewportSize({ width: 1365, height: 900 });
    await page.emulateMedia({ colorScheme: 'dark' });
    await page.goto(url);
    await page.getByRole('heading', { level: 1 }).waitFor();
    await page.evaluate(() => document.fonts.ready);
    const theme = await page.evaluate(() => {
      const root = getComputedStyle(document.documentElement);
      const body = getComputedStyle(document.body);
      return { primary: root.getPropertyValue('--primary').trim(), action: root.getPropertyValue('--action').trim(), background: body.backgroundColor, font: body.fontFamily, scheme: root.colorScheme };
    });
    check(theme.primary === '#009eff' && theme.action === '#006eaf', variant + ': incorrect theme tokens');
    check(theme.background === 'rgb(255, 255, 255)' && theme.scheme === 'light', variant + ': theme must stay light');
    check(theme.font.includes('Inter Variable'), variant + ': missing Inter');
    if (variant === 'simple') { results.push({ variant, theme, passed: true }); continue; }
    const submit = page.getByRole('button', { name: 'Sign in', exact: true });
    await submit.waitFor();
    check(await submit.getAttribute('data-slot') === 'button', variant + ': login button is not RootCX UI');
    check(await page.getByLabel('Email', { exact: true }).getAttribute('data-slot') === 'input', variant + ': login input is not RootCX UI');
    check(await submit.evaluate(el => getComputedStyle(el).backgroundColor) === 'rgb(0, 110, 175)', variant + ': action button does not use the new theme');
    await page.setViewportSize({ width: 390, height: 844 });
    check(await submit.evaluate(el => el.getBoundingClientRect().height) >= 44, variant + ': mobile controls too small');
    check(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth), variant + ': horizontal overflow');
    await page.getByRole('button', { name: 'Register', exact: true }).click();
    await page.getByLabel('Email', { exact: true }).fill(user.email);
    await page.getByLabel('Password', { exact: true }).fill('Audit-password-123');
    await page.getByLabel('Confirm password', { exact: true }).fill('Different-password');
    const before = registrations;
    await page.getByRole('button', { name: 'Create account', exact: true }).click();
    await page.getByText('Passwords do not match.').waitFor();
    check(registrations === before, variant + ': mismatched passwords reached API');
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    failLogin = true;
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await page.getByText('Wrong email or password.').waitFor();
    failLogin = false;
    await page.getByRole('button', { name: 'Sign in', exact: true }).click();
    await revealSignOut();
    if (variant === 'agent') {
      check(await page.getByRole('button', { name: 'Send message' }).getAttribute('data-slot') === 'button', 'agent: chat action is not RootCX UI');
    }
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    await page.getByRole('button', { name: 'Register', exact: true }).click();
    await page.getByLabel('Email', { exact: true }).fill(user.email);
    await page.getByLabel('Password', { exact: true }).fill('Audit-password-123');
    await page.getByLabel('Confirm password', { exact: true }).fill('Audit-password-123');
    await page.getByRole('button', { name: 'Create account', exact: true }).click();
    await revealSignOut();
    check(registrations === before + 1, variant + ': registration did not submit');
    await page.getByRole('button', { name: 'Sign out', exact: true }).click();
    passwordLoginEnabled = false;
    await page.reload();
    const sso = page.getByRole('button', { name: 'Sign in with Audit SSO' });
    await sso.waitFor();
    check(await page.getByLabel('Email', { exact: true }).count() === 0, variant + ': password form shown for SSO-only workspace');
    ssoUrl = '';
    await sso.click();
    await page.getByText('Intercepted SSO navigation').waitFor();
    check(ssoUrl.includes('/auth/oidc/audit-sso/authorize?') && ssoUrl.includes('redirect_uri=' + encodeURIComponent(url + '/')), variant + ': incorrect SSO redirect');
    providers = [];
    await page.goto(url);
    await page.getByText('No login methods available.').waitFor();
    results.push({ variant, theme, mobile: true, login: true, register: true, errors: true, sso: true, noLoginMethods: true });
  }
  check(pageErrors.length === 0, JSON.stringify(pageErrors));
  return results;
}
