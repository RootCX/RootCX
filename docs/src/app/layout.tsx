import type { Metadata } from "next";
import { Inter } from "next/font/google";
import "./globals.css";

const inter = Inter({
    subsets: ["latin"],
    variable: "--font-sans",
    display: "swap",
});

export const metadata: Metadata = {
    title: {
        default: "RootCX Documentation",
        template: "%s — RootCX Docs",
    },
    description: "Official documentation for RootCX — the open-source platform for building custom internal software and AI agents.",
    keywords: ["RootCX", "internal tools", "AI agents", "PostgreSQL", "REST API", "open source"],
    openGraph: {
        title: "RootCX Documentation",
        description: "Official documentation for RootCX — the open-source platform for building custom internal software and AI agents.",
        type: "website",
    },
};

export default function RootLayout({
    children,
}: Readonly<{
    children: React.ReactNode;
}>) {
    return (
        <html lang="en" className="dark" suppressHydrationWarning>
            <body
                suppressHydrationWarning
                className={`${inter.variable} font-sans bg-background text-foreground antialiased`}
            >
                {children}
            </body>
        </html>
    );
}
