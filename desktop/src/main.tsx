// Sets the language first: modules imported after it may work out their text as they load.
import "./application/language";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { Pointer } from "./presentation/pointer/Pointer";
import { ResearchApp } from "./presentation/shell/ResearchApp";
import "./styles/tokens.css";
import "./styles/theme.css";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: false,
      refetchOnWindowFocus: false,
    },
  },
});

// The same interface opens in the pointer window at #pointer, where it draws only the assistant's pointer.
createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      {location.hash === "#pointer" ? <Pointer /> : <ResearchApp />}
    </QueryClientProvider>
  </StrictMode>,
);
