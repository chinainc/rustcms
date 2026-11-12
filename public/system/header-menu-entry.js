/**
 * 后台头部快捷菜单：首页 / 前台 / 在线升级 / 重启系统。
 *
 * 后台前端是 umi/antd 编译产物，这里运行时注入到顶栏，风格与原生文字按钮一致。
 * 重启走已有的 `/system/api/anqi/restart`，与升级页的“立即重启服务”同一接口。
 */
(function () {
    'use strict';

    if (window.location.pathname.indexOf('/system') !== 0) {
        return;
    }
    if (/\/system\/(login|install)/.test(window.location.pathname)) {
        return;
    }

    var BAR_ID = 'rustcms-header-links';
    var STYLE_ID = 'rustcms-header-links-style';

    // 与 SPA 的请求拦截器同口径：从 localStorage/sessionStorage 的
    // `anqi-adminToken` 读令牌放进 `admin` 请求头（管理端没有会话 cookie）。
    function adminToken() {
        try {
            var raw = window.sessionStorage.getItem('anqi-adminToken')
                || window.localStorage.getItem('anqi-adminToken') || '';
            if (!raw) {
                return '';
            }
            try {
                var v = JSON.parse(raw);
                return typeof v === 'string' ? v : '';
            } catch (e) {
                return raw;
            }
        } catch (e) {
            return '';
        }
    }

    function injectStyle() {
        if (document.getElementById(STYLE_ID)) {
            return;
        }
        var style = document.createElement('style');
        style.id = STYLE_ID;
        style.textContent = [
            '#rustcms-header-links{display:flex;align-items:center;gap:18px;margin-left:auto;padding-right:16px;font-size:14px;white-space:nowrap}',
            '#rustcms-header-links a,#rustcms-header-links button{color:rgba(0,0,0,.65);background:none;border:0;padding:0;font:inherit;line-height:1.6;cursor:pointer;text-decoration:none}',
            '#rustcms-header-links a:hover,#rustcms-header-links button:hover{color:#1677ff}'
        ].join('');
        document.head.appendChild(style);
    }

    function restart() {
        if (!window.confirm('确定重启 RustCMS 服务？重启期间站点会短暂不可用。')) {
            return;
        }
        fetch('/system/api/anqi/restart', {
            method: 'POST',
            credentials: 'same-origin',
            headers: { 'Content-Type': 'application/json', 'admin': adminToken() },
            body: '{}'
        }).then(function () {
            window.alert('已发送重启指令，5 秒后刷新页面');
            setTimeout(function () { window.location.reload(); }, 5000);
        }).catch(function () {
            window.alert('重启失败，请手动重启服务');
        });
    }

    function build() {
        var bar = document.createElement('div');
        bar.id = BAR_ID;
        bar.innerHTML = [
            '<a href="/system">首页</a>',
            '<a href="/" target="_blank" rel="noopener">前台</a>',
            '<a href="/system/front-settings.html" target="_blank" rel="noopener">前台设置</a>',
            '<a href="/system/upgrade-upload.html" target="_blank" rel="noopener">在线升级</a>',
            '<button type="button" id="rustcms-header-restart">重启系统</button>'
        ].join('');
        bar.querySelector('#rustcms-header-restart').addEventListener('click', restart);
        return bar;
    }

    function tryInject() {
        if (document.getElementById(BAR_ID)) {
            return;
        }
        var header = document.querySelector(
            '.ant-pro-global-header, .ant-pro-layout-header, .ant-layout-header'
        );
        if (!header) {
            return;
        }
        injectStyle();
        var right = header.querySelector(
            '.ant-pro-global-header-right, .ant-pro-layout-header-right, .ant-pro-global-header-actions'
        );
        (right || header).appendChild(build());
    }

    function boot() {
        tryInject();
        var observer = new MutationObserver(function () {
            tryInject();
        });
        observer.observe(document.body, { childList: true, subtree: true });
        setInterval(tryInject, 1500);
    }

    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', boot);
    } else {
        boot();
    }
})();