/**
 * Hello app manifest — copied from rootCX SaaS.
 *
 * This is the real manifest.json from /src/apps/hello/.
 */
export const helloManifest = {
  appId: "hello",
  name: "Hello",
  version: "1.0.0",
  description: "Sample hello app",
  permissions: [],
  dataContract: [
    {
      entityName: "person",
      fields: [
        {
          id: "firstName",
          name: "firstName",
          label: "First name",
          type: "text",
          required: true,
          description: "A person's first name",
        },
      ],
    },
  ],
};
