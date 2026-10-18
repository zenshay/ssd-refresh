// User interaction ports. Actions do not import views.
export interface ConfirmOptions {
  title: string;
  body: string;
  warning?: string;
  okLabel: string;
}

export type Confirm = (options: ConfirmOptions) => Promise<boolean>;
