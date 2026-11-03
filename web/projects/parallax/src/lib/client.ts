/** Angular access to a parallax engine. */

import { Injectable, InjectionToken, inject, makeEnvironmentProviders } from "@angular/core";
import type { EnvironmentProviders } from "@angular/core";

import { Connection } from "./connection";

/** Where the engine runs. */
export interface ParallaxConfig {
  /** The server's WebSocket URL, such as `wss://host/ws`. A path such as `/ws` is resolved against the page. */
  readonly url: string;
  /** Where exports are posted. By default, `/export` on the same server as `url`. */
  readonly exportUrl?: string;
}

const CONFIG = new InjectionToken<ParallaxConfig>("parallax config");

/** The export endpoint beside a server's WebSocket endpoint: `wss://host/app/ws` gives `https://host/app/export`. */
export function exportUrlFor(socketUrl: string): string {
  const url = new URL(socketUrl);
  url.protocol = url.protocol === "wss:" ? "https:" : "http:";
  const base = url.pathname.replace(/\/ws\/?$/, "").replace(/\/+$/, "");
  url.pathname = `${base}/export`;
  url.search = "";
  url.hash = "";
  return url.toString();
}

/** Provides the parallax client for an application. */
export function provideParallax(config: ParallaxConfig): EnvironmentProviders {
  return makeEnvironmentProviders([{ provide: CONFIG, useValue: config }]);
}

function absolute(url: string): string {
  if (url.startsWith("ws://") || url.startsWith("wss://")) {
    return url;
  }
  const scheme = location.protocol === "https:" ? "wss:" : "ws:";
  return `${scheme}//${location.host}${url.startsWith("/") ? url : `/${url}`}`;
}

/** The application's connection to the engine, opened on first use. It needs `provideParallax`. */
@Injectable({ providedIn: "root" })
export class ParallaxClient {
  private readonly config = inject(CONFIG);
  private connecting: Promise<Connection> | null = null;

  /** The URL exports are posted to. */
  public exportUrl(): string {
    return this.config.exportUrl ?? exportUrlFor(absolute(this.config.url));
  }

  /** Returns the shared connection. */
  public connection(): Promise<Connection> {
    this.connecting ??= Connection.websocket(absolute(this.config.url));
    return this.connecting;
  }
}
