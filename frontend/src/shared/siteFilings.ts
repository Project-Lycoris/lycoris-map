import publicSecurityIcon from '@/assets/public-security-filing.png'

// Both public domains display the same filing links, as requested by the owner.
export const siteFilings = [
    {
        label: '辽ICP备2026022983号-1',
        href: 'https://beian.miit.gov.cn/',
        icon: undefined,
    },
    {
        label: '辽公网安备21030202000333号',
        href: 'https://beian.mps.gov.cn/#/query/webSearch?code=21030202000333',
        icon: publicSecurityIcon,
    },
] as const

// Leaflet owns this HTML; only fixed application data is interpolated here.
export const mapAttributionPrefix = [
    ...siteFilings.map(
        ({ label, href, icon }) =>
            `<a class="site-filing-link" href="${href}" target="_blank" rel="noopener noreferrer">${icon ? `<img src="${icon}" width="11" height="12" alt="" />` : ''}${label}</a>`,
    ),
    '<a href="https://leafletjs.com/" title="A JavaScript library for interactive maps">Leaflet</a>',
].join(' · ')
