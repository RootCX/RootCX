import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";

const params = new URLSearchParams(window.location.search);

export const windowLabel: string = getCurrentWebviewWindow().label;
export const initialProjectPath: string | null = params.get("project");
