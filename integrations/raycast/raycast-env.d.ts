/// <reference types="@raycast/api">

/* 🚧 🚧 🚧
 * This file is auto-generated from the extension's manifest.
 * Do not modify manually. Instead, update the `package.json` file.
 * 🚧 🚧 🚧 */

/* eslint-disable @typescript-eslint/ban-types */

type ExtensionPreferences = {
  /** portreaper-cli path - Leave empty to auto-detect. Set this if the binary lives somewhere unusual. */
  cliPath: string;
};

/** Preferences accessible in all the extension's commands */
declare type Preferences = ExtensionPreferences;

declare namespace Preferences {
  /** Preferences accessible in the `search-ports` command */
  export type SearchPorts = ExtensionPreferences & {};
}

declare namespace Arguments {
  /** Arguments passed to the `search-ports` command */
  export type SearchPorts = {};
}
