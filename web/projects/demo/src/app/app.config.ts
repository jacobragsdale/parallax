import { provideBrowserGlobalErrorListeners } from "@angular/core";
import type { ApplicationConfig } from "@angular/core";
import { provideParallax } from "parallax";

export const appConfig: ApplicationConfig = { providers: [provideBrowserGlobalErrorListeners(), provideParallax({ url: "/ws" })] };
