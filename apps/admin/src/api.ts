import { AdminApi, Configuration, ResponseError } from "./generated/token-toxication";
import type {
  ApiKey,
  CreateApiKeyRequest,
  CreateApiKeyResponse,
  CreateProviderAccountRequest,
  Dashboard,
  LoginResponse,
  ProviderAccount,
  RequestLog,
  UpdateApiKeyRequest,
  UpdateProviderAccountRequest,
} from "./types";
import type { ErrorResponse } from "./generated/token-toxication";

const TOKEN_KEY = "token-toxication.admin-token";

const adminApi = new AdminApi(
  new Configuration({
    basePath: "",
    accessToken: () => getStoredToken() ?? "",
  }),
);

export function getStoredToken() {
  return window.localStorage.getItem(TOKEN_KEY);
}

export function setStoredToken(token: string) {
  window.localStorage.setItem(TOKEN_KEY, token);
}

export function clearStoredToken() {
  window.localStorage.removeItem(TOKEN_KEY);
}

async function callApi<T>(operation: Promise<T>): Promise<T> {
  try {
    return await operation;
  } catch (error) {
    throw await normalizeApiError(error);
  }
}

async function normalizeApiError(error: unknown): Promise<Error> {
  if (error instanceof ResponseError) {
    let message = `${error.response.status} ${error.response.statusText}`;
    try {
      const payload = (await error.response.clone().json()) as Partial<ErrorResponse> & {
        message?: string;
      };
      message = payload.error?.message ?? payload.message ?? message;
    } catch {
      // Keep the status-derived message for non-JSON errors.
    }
    return new Error(message);
  }

  if (error instanceof Error) {
    return error;
  }

  return new Error("Request failed");
}

function unwrapResult<T>(payload: T | ErrorResponse): T {
  if (isErrorResponse(payload)) {
    throw new Error(payload.error.message);
  }

  return payload;
}

function isErrorResponse(payload: unknown): payload is ErrorResponse {
  return (
    typeof payload === "object" &&
    payload !== null &&
    "error" in payload &&
    typeof (payload as { error?: unknown }).error === "object"
  );
}

export const api = {
  async login(username: string, password: string): Promise<LoginResponse> {
    const response = await callApi(adminApi.adminLogin({ body: { username, password } }));
    return unwrapResult(response);
  },

  async logout() {
    await callApi(adminApi.adminLogout());
  },

  async dashboard(): Promise<Dashboard> {
    const response = await callApi(adminApi.adminDashboard());
    return unwrapResult(response);
  },

  async apiKeys(): Promise<ApiKey[]> {
    const response = await callApi(adminApi.listApiKeys());
    return [...response.data];
  },

  async createApiKey(payload: CreateApiKeyRequest): Promise<CreateApiKeyResponse> {
    const response = await callApi(adminApi.createApiKey({ body: payload }));
    return unwrapResult(response);
  },

  async updateApiKey(id: string, payload: UpdateApiKeyRequest): Promise<ApiKey> {
    const response = await callApi(adminApi.updateApiKey({ id, body: payload }));
    return unwrapResult(response).data;
  },

  async deleteApiKey(id: string) {
    await callApi(adminApi.deleteApiKeyRaw({ id }));
  },

  async providerAccounts(): Promise<ProviderAccount[]> {
    const response = await callApi(adminApi.listProviderAccounts());
    return [...response.data];
  },

  async createProviderAccount(payload: CreateProviderAccountRequest): Promise<ProviderAccount> {
    const response = await callApi(adminApi.createProviderAccount({ body: payload }));
    return unwrapResult(response).data;
  },

  async updateProviderAccount(
    id: string,
    payload: UpdateProviderAccountRequest,
  ): Promise<ProviderAccount> {
    const response = await callApi(adminApi.updateProviderAccount({ id, body: payload }));
    return unwrapResult(response).data;
  },

  async deleteProviderAccount(id: string) {
    await callApi(adminApi.deleteProviderAccountRaw({ id }));
  },

  async requestLogs(limit = 50): Promise<RequestLog[]> {
    const response = await callApi(adminApi.listRequestLogs({ limit }));
    return [...response.data];
  },
};
