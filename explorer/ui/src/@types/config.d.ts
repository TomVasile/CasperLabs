// Declared so we can get it from the `window` object.
interface Window {
  origin: string;
  config: Config;
}

interface Config {
  auth0: Auth0Config;
  auth: {
    mock: {
      enabled: boolean;
    };
  };
  grpc: {
    url: string | undefined;
  };
}

interface Auth0Config {
  domain: string;
  clientId: string;
}
