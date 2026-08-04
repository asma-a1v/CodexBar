import type {
  CookieSourceOption,
  CredentialStorageStatus,
  ProviderDetail,
  RegionOption,
} from "../../../types/bridge";

export interface ProviderDetailPaneState {
  detail: ProviderDetail | null;
  cookieOptions: CookieSourceOption[];
  regionOptions: RegionOption[];
  credentialStatus: CredentialStorageStatus | null;
  credentialRevision: number;
  tokenProviderIds: Set<string>;
  loading: boolean;
  error: string | null;
  busy: boolean;
  gatewayDraft: string;
  gatewayError: string | null;
  syncedProviderId: string | null;
  syncedGatewayUrl: string;
}

/** Clustered actions — load bundle / ui flags / gateway. */
export type ProviderDetailPaneAction =
  // load bundle
  | { type: "LOAD_START" }
  | {
      type: "LOAD_SUCCESS";
      detail: ProviderDetail;
      cookieOptions: CookieSourceOption[];
      regionOptions: RegionOption[];
      credentialStatus: CredentialStorageStatus;
    }
  | { type: "LOAD_ERROR"; error: string }
  | { type: "LOAD_FINISH" }
  | { type: "LOAD_CLEAR"; mode: "switch" | "empty" }
  // busy / error / credentialRevision / tokenProviders
  | { type: "SET_BUSY"; busy: boolean }
  | { type: "SET_ERROR"; error: string | null }
  | { type: "BUMP_CREDENTIAL_REVISION" }
  | { type: "SET_TOKEN_PROVIDERS"; ids: Iterable<string> }
  // gateway
  | {
      type: "SYNC_PROPS";
      providerId: string | null;
      wayfinderGatewayUrl: string;
    }
  | { type: "SET_GATEWAY_DRAFT"; draft: string }
  | { type: "SAVE_GATEWAY_START" }
  | { type: "SAVE_GATEWAY_ERROR"; error: string };

export function initialState(
  wayfinderGatewayUrl: string,
  providerId?: string | null,
): ProviderDetailPaneState {
  return {
    detail: null,
    cookieOptions: [],
    regionOptions: [],
    credentialStatus: null,
    credentialRevision: 0,
    tokenProviderIds: new Set(),
    loading: false,
    error: null,
    busy: false,
    gatewayDraft: wayfinderGatewayUrl,
    gatewayError: null,
    syncedProviderId: providerId ?? null,
    syncedGatewayUrl: wayfinderGatewayUrl,
  };
}

export function providerDetailPaneReducer(
  state: ProviderDetailPaneState,
  action: ProviderDetailPaneAction,
): ProviderDetailPaneState {
  switch (action.type) {
    case "LOAD_START":
      return { ...state, loading: true, error: null };
    case "LOAD_SUCCESS":
      return {
        ...state,
        error: null,
        detail: action.detail,
        cookieOptions: action.cookieOptions,
        regionOptions: action.regionOptions,
        credentialStatus: action.credentialStatus,
      };
    case "LOAD_ERROR":
      return {
        ...state,
        error: action.error,
        detail: null,
        cookieOptions: [],
        regionOptions: [],
        credentialStatus: null,
      };
    case "LOAD_FINISH":
      return { ...state, loading: false };
    case "LOAD_CLEAR":
      return {
        ...state,
        detail: null,
        cookieOptions: [],
        regionOptions: [],
        ...(action.mode === "switch" ? { credentialStatus: null } : {}),
      };
    case "SET_BUSY":
      return { ...state, busy: action.busy };
    case "SET_ERROR":
      return { ...state, error: action.error };
    case "BUMP_CREDENTIAL_REVISION":
      return { ...state, credentialRevision: state.credentialRevision + 1 };
    case "SET_TOKEN_PROVIDERS":
      return { ...state, tokenProviderIds: new Set(action.ids) };
    case "SYNC_PROPS": {
      if (
        action.providerId === state.syncedProviderId &&
        action.wayfinderGatewayUrl === state.syncedGatewayUrl
      ) {
        return state;
      }
      return {
        ...state,
        syncedProviderId: action.providerId,
        syncedGatewayUrl: action.wayfinderGatewayUrl,
        gatewayDraft:
          action.providerId === "wayfinder"
            ? action.wayfinderGatewayUrl
            : state.gatewayDraft,
        gatewayError: null,
      };
    }
    case "SET_GATEWAY_DRAFT":
      return { ...state, gatewayDraft: action.draft };
    case "SAVE_GATEWAY_START":
      return { ...state, busy: true, gatewayError: null };
    case "SAVE_GATEWAY_ERROR":
      return { ...state, gatewayError: action.error };
    default:
      return state;
  }
}
