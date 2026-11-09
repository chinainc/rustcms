/* =========================================================
   商家工作台 · 交互
   数据卡切换 / 询盘列表 / 待办 / 侧栏高亮 / 轻提示
   ========================================================= */
(function () {
  'use strict';

  /* ---------- 演示数据 ---------- */
  var METRICS = {
    realtime: [
      { label: '搜索曝光次数', value: '8171', delta: '11.7%', up: true },
      { label: '搜索点击次数', value: '161', delta: '17.5%', up: true },
      { label: '店铺访问人数', value: '227', delta: '51.3%', up: true },
      { label: '询盘个数', value: '1', delta: '75%', up: false },
      { label: '访问人数', value: '1', delta: '75%', up: false },
      { label: 'IM咨询人数', value: '12', delta: '14.3%', up: false }
    ],
    shop: [
      { label: '商品访客数', value: '1,382', delta: '8.4%', up: true },
      { label: '商品浏览量', value: '3,915', delta: '12.1%', up: true },
      { label: '收藏人数', value: '76', delta: '3.2%', up: true },
      { label: '加购人数', value: '41', delta: '2.6%', up: false },
      { label: '询盘转化率', value: '1.8%', delta: '0.4%', up: true },
      { label: '客单价', value: 'US$356', delta: '5.1%', up: false }
    ],
    market: [
      { label: '营销曝光量', value: '12,640', delta: '23.4%', up: true },
      { label: '点击量', value: '486', delta: '15.2%', up: true },
      { label: '营销花费', value: 'US$128.40', delta: '6.8%', up: false },
      { label: '营销询盘', value: '9', delta: '28.6%', up: true },
      { label: '点击率', value: '3.85%', delta: '1.2%', up: false },
      { label: 'ROI', value: '6.4', delta: '0.8%', up: true }
    ]
  };

  var RFQ = [
    {
      flag: 'us',
      title: '2026 Hot Selling Bandage Gown For Women Prom Party Dress',
      sub: 'Hello,I am currently developing my own fashion brand,specializes in evening dresses.',
      price: 'US$20.00/Piece',
      time: '5 小时前',
      seats: '剩余 8 个报价席位'
    },
    {
      flag: 'de',
      title: 'Maxi Dress Ladies',
      sub: "I'm looking for maxi, Product specifications:Material:Spandex/Elastane…",
      price: 'US$2,000.00/Piece',
      time: '10 小时前',
      seats: '剩余 2 个报价席位'
    },
    {
      flag: 'gb',
      title: 'Yellow Abstract Made Little Occasional Maxi Dress',
      sub: 'Made in China, custom sizes and labels available for bulk order.',
      price: 'US$2.00/Piece',
      time: '2 小时前',
      seats: '剩余 2 个报价席位'
    },
    {
      flag: 'fr',
      title: 'Strapless Solid Color Hourglass Waist Dress Is Slim Fit',
      sub: 'Made in China, need 500pcs per color, please quote with shipping.',
      price: 'US$18.50/Piece',
      time: '13 小时前',
      seats: '剩余 5 个报价席位'
    },
    {
      flag: 'au',
      title: 'Wholesale Custom Trench Coat Women Double Breasted',
      sub: 'Looking for 300pcs per style, OEM accepted, samples needed first.',
      price: 'US$26.00/Piece',
      time: '1 小时前',
      seats: '剩余 6 个报价席位'
    }
  ];

  var TODO = [
    { label: '未读人数', value: 0 },
    { label: '询盘回复', value: 3 },
    { label: '可营销客户', value: 20 },
    { label: '日超期', value: 0 },
    { label: '逾期待办', value: 0 },
    { label: '客诉不通过', value: 0 }
  ];

  /* ---------- 菜单结构 ---------- */
  var MENU = [
    { key: 'home', name: '首页', icon: 'i-home' },
    { key: 'common', name: '常用', icon: 'i-star', children: [
      { key: 'favorite', name: '我的收藏' },
      { key: 'recent', name: '最近访问' },
      { key: 'shortcut', name: '自定义快捷入口' }
    ] },
    { key: 'shop', name: '店铺管理', icon: 'i-shop', children: [
      { key: 'decorate', name: '装修店铺' },
      { key: 'manage', name: '管理店铺' },
      { key: 'compliance', name: '合规中心' },
      { key: 'shop-setting', name: '店铺设置' },
      { key: 'company', name: '管理公司信息' },
      { key: 'tariff', name: '设置交易品关税' },
      { key: 'trade-record', name: '贸易记录' }
    ] },
    { key: 'opportunity', name: '商机管理', icon: 'i-target', children: [
      { key: 'product-publish', name: '发布商品' },
      { key: 'product-list', name: '商品管理' },
      { key: 'product-group', name: '商品分组' },
      { key: 'showcase', name: '橱窗产品' },
      { key: 'rfq-market', name: 'RFQ 市场' }
    ] },
    { key: 'media', name: '媒体中心', icon: 'i-play', dot: true, children: [
      { key: 'video', name: '视频中心' },
      { key: 'image-bank', name: '图片银行' },
      { key: 'live', name: '直播间' },
      { key: 'material', name: '素材管理' }
    ] },
    { key: 'data', name: '数据参谋', icon: 'i-chart', children: [
      { key: 'overview', name: '数据纵横' },
      { key: 'shop-analysis', name: '店铺分析' },
      { key: 'product-analysis', name: '商品分析' },
      { key: 'traffic', name: '流量分析' },
      { key: 'visitor', name: '访客画像' }
    ] },
    { key: 'marketing', name: '营销中心', icon: 'i-mega', children: [
      { key: 'p4p', name: '外贸直通车' },
      { key: 'top-show', name: '顶级展位' },
      { key: 'star-shop', name: '明星店铺' },
      { key: 'coupon', name: '优惠券' },
      { key: 'campaign', name: '营销活动' }
    ] },
    { key: 'activity', name: '活动中心', icon: 'i-gift', children: [
      { key: 'platform', name: '平台活动' },
      { key: 'signup', name: '报名管理' },
      { key: 'season', name: '九月采购节' },
      { key: 'sample', name: '样品中心' }
    ] },
    { key: 'im', name: '商机沟通', icon: 'i-chat', children: [
      { key: 'inquiry', name: '询盘管理' },
      { key: 'tm', name: 'TradeManager' },
      { key: 'message', name: '旺铺留言' },
      { key: 'edm', name: '邮件营销' }
    ] },
    { key: 'crm', name: '客户管理', icon: 'i-users', children: [
      { key: 'customer-list', name: '客户列表' },
      { key: 'segment', name: '客户分群' },
      { key: 'follow', name: '跟进记录' },
      { key: 'crm-setting', name: 'CRM 设置' }
    ] },
    { key: 'trade', name: '交易管理', icon: 'i-cart', children: [
      { key: 'order', name: '订单管理' },
      { key: 'draft', name: '起草订单' },
      { key: 'dispute', name: '纠纷处理' },
      { key: 'review', name: '评价管理' },
      { key: 'refund', name: '退款管理' }
    ] },
    { key: 'export', name: '出口服务', icon: 'i-truck', children: [
      { key: 'onetouch', name: '一达通' },
      { key: 'customs', name: '报关服务' },
      { key: 'tax-refund', name: '退税服务' },
      { key: 'lc', name: '信用证' }
    ] },
    { key: 'logistics', name: '物流服务', icon: 'i-box', children: [
      { key: 'plan', name: '物流方案' },
      { key: 'shipping', name: '发货管理' },
      { key: 'freight', name: '运费模板' },
      { key: 'overseas', name: '海外仓' }
    ] },
    { key: 'fund', name: '资金管理', icon: 'i-wallet', children: [
      { key: 'balance', name: '账户余额' },
      { key: 'statement', name: '收支明细' },
      { key: 'withdraw', name: '提现' },
      { key: 'credit', name: '信用保障额度' }
    ] },
    { key: 'verify', name: '认证中心', icon: 'i-badge', children: [
      { key: 'corp-verify', name: '企业认证' },
      { key: 'gold', name: '金牌认证' },
      { key: 'inspection', name: '检测认证' },
      { key: 'cert', name: '证书管理' }
    ] },
    { key: 'grow', name: '商家成长', icon: 'i-growth', children: [
      { key: 'learn', name: '学习中心' },
      { key: 'task', name: '成长任务' },
      { key: 'level', name: '商家分层' },
      { key: 'service-market', name: '服务市场' }
    ] },
    { key: 'assistant', name: '生意助手', icon: 'i-robot', dot: true, children: [
      { key: 'ai-select', name: '智能选品' },
      { key: 'ai-title', name: '智能标题' },
      { key: 'ai-marketing', name: '智能营销' },
      { key: 'ai-report', name: '诊断报告' }
    ] },
    { key: 'service', name: '服务中心', icon: 'i-headset', children: [
      { key: 'online', name: '在线客服' },
      { key: 'ticket', name: '工单系统' },
      { key: 'help-center', name: '帮助中心' },
      { key: 'feedback', name: '意见反馈' }
    ] }
  ];

  var current = { group: 'home', page: 'home' };

  function groupOf(key) {
    for (var i = 0; i < MENU.length; i++) {
      if (MENU[i].key === key) return MENU[i];
    }
    return null;
  }

  /* ---------- 导航渲染 ---------- */
  function renderNav() {
    var nav = document.getElementById('nav');
    if (!nav) return;
    nav.innerHTML = MENU.map(function (m) {
      return '' +
        '<li>' +
          '<a class="nav__item" href="#" data-group="' + m.key + '">' +
            '<svg class="ico"><use href="#' + m.icon + '"/></svg>' +
            '<span>' + m.name + '</span>' +
            (m.dot ? '<i class="nav__dot"></i>' : '') +
            (m.children ? '<svg class="caret"><use href="#i-arrow"/></svg>' : '') +
          '</a>' +
        '</li>';
    }).join('');
  }

  function paintNav() {
    var items = document.querySelectorAll('.nav__item');
    Array.prototype.forEach.call(items, function (it) {
      var m = groupOf(it.dataset.group);
      var on = !!m && it.dataset.group === current.group;
      it.classList.toggle('is-active', on);
      it.classList.toggle('is-open', on && !!(m && m.children));
    });
  }

  function renderSubnav(m) {
    var box = document.getElementById('subnav');
    var title = document.getElementById('subnavTitle');
    var list = document.getElementById('subnavList');
    if (!box) return;
    if (!m || !m.children) {
      box.hidden = true;
      return;
    }
    title.textContent = m.name;
    list.innerHTML = m.children.map(function (c) {
      return '<li><a class="subnav__item" href="#" data-page="' + c.key + '">' + c.name + '</a></li>';
    }).join('');
    box.hidden = false;
  }

  function paintSubnav() {
    var items = document.querySelectorAll('.subnav__item');
    Array.prototype.forEach.call(items, function (it) {
      it.classList.toggle('is-active', it.dataset.page === current.page);
    });
  }

  function showDashboard() {
    document.getElementById('dashboard').hidden = false;
    document.getElementById('pageview').hidden = true;
  }

  function showPage(m, child) {
    document.getElementById('pageCrumb').textContent = m.name + ' / ' + child.name;
    document.getElementById('pageTitle').textContent = child.name;
    document.getElementById('pageIco').innerHTML =
      '<svg class="ico"><use href="#' + m.icon + '"/></svg>';
    document.getElementById('dashboard').hidden = true;
    document.getElementById('pageview').hidden = false;
  }

  function openGroup(key) {
    var m = groupOf(key);
    if (!m) return;
    current.group = key;
    paintNav();
    if (!m.children) {
      current.page = key;
      renderSubnav(null);
      showDashboard();
      return;
    }
    renderSubnav(m);
    current.page = m.children[0].key;
    paintSubnav();
    showPage(m, m.children[0]);
  }

  function openPage(key) {
    var m = groupOf(current.group);
    if (!m || !m.children) return;
    var child = null;
    m.children.forEach(function (c) { if (c.key === key) child = c; });
    if (!child) return;
    current.page = child.key;
    paintSubnav();
    showPage(m, child);
  }

  /* ---------- 渲染 ---------- */
  function renderMetrics(key) {
    var box = document.getElementById('metrics');
    if (!box) return;
    var list = METRICS[key] || METRICS.realtime;
    box.innerHTML = list.map(function (m) {
      var arrow = m.up ? '↑' : '↓';
      var cls = m.up ? 'up' : 'down';
      return '' +
        '<div class="metric">' +
          '<p class="metric__label">' + m.label + '</p>' +
          '<p class="metric__value">' +
            '<b>' + m.value + '</b>' +
            '<span class="metric__delta metric__delta--' + cls + '">' + arrow + ' ' + m.delta + '</span>' +
          '</p>' +
        '</div>';
    }).join('');
  }

  function renderRFQ() {
    var box = document.getElementById('rfq');
    if (!box) return;
    box.innerHTML = RFQ.map(function (r, i) {
      return '' +
        '<li class="rfq__row" data-idx="' + i + '">' +
          '<span class="flag flag--' + r.flag + '"></span>' +
          '<div class="rfq__main">' +
            '<a class="rfq__title" href="#">' + r.title + '</a>' +
            '<p class="rfq__sub">' + r.sub + '</p>' +
          '</div>' +
          '<span class="rfq__price">' + r.price + '</span>' +
          '<span class="rfq__time">' + r.time + '</span>' +
          '<span class="rfq__seats">' + r.seats + '</span>' +
          '<div class="rfq__acts">' +
            '<button class="btn btn--ghost" type="button" data-act="assist">辅助报价</button>' +
            '<button class="btn btn--solid" type="button" data-act="quote">去报价</button>' +
          '</div>' +
        '</li>';
    }).join('');
  }

  function renderTodo() {
    var box = document.getElementById('todo');
    if (!box) return;
    box.innerHTML = TODO.map(function (t) {
      var hot = t.value > 0 ? ' is-hot' : '';
      return '<p class="todo__item"><span>' + t.label + '</span><b class="' +
        hot.trim() + '">' + t.value + '</b></p>';
    }).join('');
  }

  /* ---------- 轻提示 ---------- */
  var toastTimer = null;
  function toast(msg) {
    var el = document.getElementById('toast');
    if (!el) return;
    el.textContent = msg;
    el.classList.add('is-show');
    clearTimeout(toastTimer);
    toastTimer = setTimeout(function () {
      el.classList.remove('is-show');
    }, 1800);
  }

  /* ---------- 事件 ---------- */
  function bind() {
    // 数据卡切换
    var tabs = document.querySelectorAll('.tabs__item');
    Array.prototype.forEach.call(tabs, function (tab) {
      tab.addEventListener('click', function () {
        Array.prototype.forEach.call(tabs, function (t) { t.classList.remove('is-active'); });
        tab.classList.add('is-active');
        renderMetrics(tab.dataset.tab);
      });
    });

    // 一级菜单 / 二级菜单（事件委托）
    document.addEventListener('click', function (e) {
      var nav = e.target.closest ? e.target.closest('.nav__item') : null;
      if (nav) {
        e.preventDefault();
        openGroup(nav.dataset.group);
        return;
      }
      var sub = e.target.closest ? e.target.closest('.subnav__item') : null;
      if (sub) {
        e.preventDefault();
        openPage(sub.dataset.page);
      }
    });

    // 询盘操作
    document.addEventListener('click', function (e) {
      var btn = e.target.closest ? e.target.closest('[data-act]') : null;
      if (!btn) return;
      e.preventDefault();
      var row = btn.closest('.rfq__row');
      var idx = row ? Number(row.dataset.idx) : -1;
      var title = idx >= 0 && RFQ[idx] ? RFQ[idx].title : '';
      toast((btn.dataset.act === 'quote' ? '进入报价：' : '辅助报价：') + title);
    });

    // 演示动作
    document.addEventListener('click', function (e) {
      var el = e.target.closest ? e.target.closest('.ai__btn, .promo__btn') : null;
      if (!el) return;
      toast(el.classList.contains('ai__btn') ? 'Accio Work 即将开放体验' : '生意助手新版已开启');
    });
  }

  /* ---------- 启动 ---------- */
  renderNav();
  paintNav();
  renderMetrics('realtime');
  renderRFQ();
  renderTodo();
  bind();
})();
