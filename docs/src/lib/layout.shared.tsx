import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: 'bkgcrewcli',
    },
    links: [
      {
        text: 'GitHub',
        url: 'https://github.com/bkg-zig/bkg-crew-cli',
      },
    ],
  };
}
