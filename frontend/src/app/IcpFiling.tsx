import { createContext, useContext, useLayoutEffect, useRef, useState, type ReactNode } from 'react'
import { useViewportSnapshot } from '@/layouts/viewport'
import './icp-filing.css'

const FilingInset = createContext(0)
export const useFilingInset = () => useContext(FilingInset)

export function IcpFiling({ children }: { children: ReactNode }) {
    const enabled = ['lycoris-map.cn', 'www.lycoris-map.cn'].includes(window.location.hostname)
    const footer = useRef<HTMLElement>(null)
    const [height, setHeight] = useState(enabled ? 26 : 0)
    const viewport = useViewportSnapshot()

    useLayoutEffect(() => {
        const element = footer.current
        if (!element) return
        // Reserve the actual footer and safe-area height in the shared map geometry.
        // Otherwise the footer would cover sheet actions on phones and after rotation.
        const measure = () => setHeight(Math.ceil(element.getBoundingClientRect().height))
        measure()
        const observer = new ResizeObserver(measure)
        observer.observe(element)
        return () => observer.disconnect()
    }, [])

    return (
        <FilingInset value={height}>
            <div style={{ paddingBottom: height }}>
                {children}
                {enabled && (
                    <footer
                        ref={footer}
                        className="icp-filing"
                        style={{ top: viewport.bottom - height }}
                    >
                        <a
                            href="https://beian.miit.gov.cn/"
                            target="_blank"
                            rel="noopener noreferrer"
                        >
                            辽ICP备2026022983号-1
                        </a>
                    </footer>
                )}
            </div>
        </FilingInset>
    )
}
