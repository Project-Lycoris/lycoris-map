import { Fragment } from 'react'
import { siteFilings } from '@/shared/siteFilings'
import '@/shared/site-filings.css'

// Administration has no map attribution control, so retain its filing links here.
export function SiteFilingFooter() {
    return (
        <footer className="site-filings-footer">
            {siteFilings.map(({ label, href, icon }, index) => (
                <Fragment key={href}>
                    {index > 0 && ' · '}
                    <a
                        className="site-filing-link"
                        href={href}
                        target="_blank"
                        rel="noopener noreferrer"
                    >
                        {icon && <img src={icon} width={11} height={12} alt="" />}
                        {label}
                    </a>
                </Fragment>
            ))}
        </footer>
    )
}
