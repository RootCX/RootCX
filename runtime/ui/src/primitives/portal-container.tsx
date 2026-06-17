import * as React from "react";

export const PortalContainerContext = React.createContext<HTMLElement | null>(null);
export const usePortalContainer = () => React.useContext(PortalContainerContext);
