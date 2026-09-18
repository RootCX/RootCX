import { Page, PageHeader, PageHeading, PageTitle, PageDescription } from "@rootcx/ui";

export default function App() {
  return (
    <main className="mx-auto w-full max-w-5xl">
      <Page>
        <PageHeader>
          <PageHeading>
            <PageTitle>__APP_ID__</PageTitle>
            <PageDescription>Get started by editing src/App.tsx</PageDescription>
          </PageHeading>
        </PageHeader>
      </Page>
    </main>
  );
}
