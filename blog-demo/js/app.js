/* ============================================================
   墨与游 · 应用逻辑
   纯前端 SPA · localStorage 模拟后端
   ============================================================ */
(function () {
  "use strict";

  /* ---------- 工具 ---------- */
  const $ = (sel, el = document) => el.querySelector(sel);
  const $$ = (sel, el = document) => Array.from(el.querySelectorAll(sel));
  const view = $("#view");

  const uid = () => Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
  const escapeHtml = (s = "") =>
    String(s).replace(/[&<>"']/g, (c) => ({
      "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;",
    }[c]));

  function toast(msg) {
    const t = $("#toast");
    t.textContent = msg;
    t.hidden = false;
    requestAnimationFrame(() => t.classList.add("show"));
    clearTimeout(toast._t);
    toast._t = setTimeout(() => {
      t.classList.remove("show");
      setTimeout(() => (t.hidden = true), 200);
    }, 1800);
  }

  function formatDate(ts) {
    const d = new Date(ts);
    const now = Date.now();
    const diff = (now - ts) / 1000;
    if (diff < 60) return "刚刚";
    if (diff < 3600) return Math.floor(diff / 60) + " 分钟前";
    if (diff < 86400) return Math.floor(diff / 3600) + " 小时前";
    if (diff < 604800) return Math.floor(diff / 86400) + " 天前";
    return `${d.getMonth() + 1}月${d.getDate()}日`;
  }

  // 根据名字生成稳定的头像配色
  const AVATAR_COLORS = ["#5bb89e", "#f08362", "#6c8cff", "#e0a23c", "#9b6cd8", "#3f9c82"];
  function avatarColor(name = "") {
    let h = 0;
    for (let i = 0; i < name.length; i++) h = (h * 31 + name.charCodeAt(i)) >>> 0;
    return AVATAR_COLORS[h % AVATAR_COLORS.length];
  }
  function avatar(name) {
    const ch = (name || "?").trim().charAt(0).toUpperCase();
    return `<span class="avatar" style="background:${avatarColor(name)}">${escapeHtml(ch)}</span>`;
  }

  /* ---------- 数据层（localStorage 模拟后端） ---------- */
  const KEY = "moyou-db-v1";
  const SESSION = "moyou-session";

  function seed() {
    const now = Date.now();
    return {
      users: [
        { id: "u_admin", username: "小墨", password: "1234", bio: "爱写科幻的玩家 · 站长", createdAt: now - 86400000 * 9 },
        { id: "u_luna", username: "Luna", password: "1234", bio: "独立游戏爱好者", createdAt: now - 86400000 * 5 },
      ],
      posts: [
        {
          id: "p1", type: "novel", title: "星海拾遗 · 序章",
          excerpt: "当第一束人造光抵达深空，她才意识到，自己成了宇宙里最后一名写信的人。",
          cover: "📚", emoji: "📖",
          authorId: "u_admin", createdAt: now - 3600000 * 5, likes: 42, views: 318,
          body: "星海拾遗 · 序章\n\n当第一束人造光抵达深空，她才意识到，自己成了宇宙里最后一名写信的人。\n\n舷窗外的星河缓慢旋转，像一卷被风翻动的旧书。她把钢笔搁在膝上，看着墨水在失重里凝成一颗小小的、漆黑的星球。\n\n“致 ——”\n\n她写下这个字，停了很久。\n\n致谁呢？收信的人都已化作数据，躺在某个不再有人维护的服务器里。可她还是写。因为只要还在写，这艘船就还不算空。\n\n（未完待续……）",
        },
        {
          id: "p2", type: "novel", title: "巷子尽头的旧书店",
          excerpt: "推开门的瞬间，风铃响了。老板抬起头，眼神像一本合上的书。",
          cover: "🏛️", emoji: "📕",
          authorId: "u_luna", createdAt: now - 3600000 * 26, likes: 18, views: 154,
          body: "巷子尽头的旧书店\n\n推开门的瞬间，风铃响了。老板抬起头，眼神像一本合上的书。\n\n“找什么？”\n\n“一本……我也不确定的书。”\n\n他笑了笑，那种笑里藏着很多年。“那你来对地方了。”\n\n书架一直延伸到天花板的阴影里，空气是旧纸和木头的味道。我随手抽出一本，扉页上有人用钢笔写着：愿你翻到的每一页，都是答案。\n\n我买下了它。回家的路上，雨刚停。",
        },
        {
          id: "p3", type: "game", title: "《风之旅人》通关随笔：一场无声的朝圣",
          excerpt: "没有血条，没有分数，只有一座山，和一条向前延伸的路。",
          cover: "🏜️", emoji: "🎮",
          authorId: "u_luna", createdAt: now - 3600000 * 8, likes: 67, views: 502,
          body: "《风之旅人》通关随笔\n\n没有血条，没有分数，只有一座山，和一条向前延伸的路。\n\n我花了两个小时走完它，屏幕暗下时，发现自己一直屏着呼吸。\n\n最动人的不是画面，而是中途遇到的那个陌生人——没有文字，没有语音，只有一声声清脆的鸣响。我们靠在一起取暖，一起穿过沙暴。最后在山顶分别时，我甚至不知道ta是谁。\n\n这大概就是游戏作为“艺术”的样子：它不告诉你该感受什么，只让你去感受。",
        },
        {
          id: "p4", type: "game", title: "推荐三款适合周末下午的治愈系小游戏",
          excerpt: "泡一杯茶，选一款，把整个下午交给它。",
          cover: "🍵", emoji: "🎲",
          authorId: "u_admin", createdAt: now - 3600000 * 30, likes: 33, views: 276,
          body: "周末治愈系推荐\n\n泡一杯茶，选一款，把整个下午交给它。\n\n1. 《星露谷物语》—— 种地、钓鱼、和邻居谈恋爱，最温柔的“数字乡愁”。\n2. 《Unpacking》—— 拆箱子整理房间，竟然意外地解压，像在整理自己的人生。\n3. 《Alba》—— 小女孩在小岛上拍野生动物，每张照片都在让世界变好一点点。\n\n没有肝度，没有焦虑，只有你和你自己的节奏。",
        },
      ],
    };
  }

  const db = {
    load() {
      try {
        const raw = localStorage.getItem(KEY);
        if (raw) return JSON.parse(raw);
      } catch (e) {}
      const s = seed();
      this.save(s);
      return s;
    },
    save(data) {
      localStorage.setItem(KEY, JSON.stringify(data));
    },
    reset() {
      localStorage.removeItem(KEY);
      localStorage.removeItem(SESSION);
      return this.load();
    },
  };

  let state = db.load();

  function currentUser() {
    const sid = localStorage.getItem(SESSION);
    if (!sid) return null;
    return state.users.find((u) => u.id === sid) || null;
  }
  function setSession(id) {
    if (id) localStorage.setItem(SESSION, id);
    else localStorage.removeItem(SESSION);
  }
  function userById(id) {
    return state.users.find((u) => u.id === id) || { username: "匿名", bio: "" };
  }
  function postsByType(type) {
    return state.posts
      .filter((p) => (type ? p.type === type : true))
      .sort((a, b) => b.createdAt - a.createdAt);
  }

  /* ---------- 渲染：导航高亮 + 用户区 ---------- */
  function renderChrome(route) {
    $$(".nav-link").forEach((a) => {
      const r = a.dataset.route;
      a.classList.toggle("nav-link--active", r === route);
    });
    const u = currentUser();
    const area = $("#userArea");
    if (u) {
      area.innerHTML = `
        ${avatar(u.username)}
        <span class="user-name">${escapeHtml(u.username)}</span>
        <button class="btn btn--ghost btn--sm" id="logoutBtn">登出</button>`;
      $("#logoutBtn").addEventListener("click", () => {
        setSession(null);
        toast("已登出，下次再来玩~");
        renderChrome();
        router();
      });
    } else {
      area.innerHTML = `<button class="btn btn--primary btn--sm" id="openAuth">登录 / 注册</button>`;
      $("#openAuth").addEventListener("click", openAuth);
    }
  }

  /* ---------- 视图组件 ---------- */
  function postCard(p) {
    const author = userById(p.authorId);
    return `
      <article class="card" data-id="${p.id}">
        <div class="card-cover cover--${p.type}">${p.emoji || p.cover || "📝"}</div>
        <span class="tag tag--${p.type}">${p.type === "novel" ? "✍️ 小说" : "🎮 游戏"}</span>
        <h3 class="card-title">${escapeHtml(p.title)}</h3>
        <p class="card-excerpt">${escapeHtml(p.excerpt)}</p>
        <div class="card-meta">
          <span class="author">${avatar(author.username)} ${escapeHtml(author.username)}</span>
          <span>
            <span class="stat">👁 ${p.views}</span>
            &nbsp;<span class="stat">❤ ${p.likes}</span>
            &nbsp;· ${formatDate(p.createdAt)}
          </span>
        </div>
      </article>`;
  }

  function homeView() {
    const novels = postsByType("novel").slice(0, 3);
    const games = postsByType("game").slice(0, 3);
    return `
      <section class="view-enter">
        <div class="hero">
          <div class="hero-text">
            <h1 class="hero-title">在这里，<em>写故事</em>，也<em>玩游戏</em>。</h1>
            <p class="hero-desc">墨与游是一个轻松的小社区，分享你写的小说、玩过的游戏。<br/>没有压力，只有文字与快乐。</p>
            <button class="btn btn--primary" id="heroPublish">${currentUser() ? "✍️ 写点什么" : "开始创作（先登录）"}</button>
          </div>
          <div class="hero-emoji">🕹️</div>
        </div>

        <div class="section">
          <div class="section-head">
            <h2 class="section-title"><span class="dot dot--novel"></span>最新小说</h2>
            <a class="link-more" href="#/novel">查看全部 →</a>
          </div>
          <div class="grid">${novels.map(postCard).join("")}</div>
        </div>

        <div class="section">
          <div class="section-head">
            <h2 class="section-title"><span class="dot dot--game"></span>最新游戏</h2>
            <a class="link-more" href="#/game">查看全部 →</a>
          </div>
          <div class="grid">${games.map(postCard).join("")}</div>
        </div>
      </section>`;
  }

  function listView(type) {
    const label = type === "novel" ? "小说" : "游戏";
    const posts = postsByType(type);
    const total = state.posts.filter((p) => p.type === type).length;
    const authorCount = new Set(posts.map((p) => p.authorId)).size;
    return `
      <section class="view-enter">
        <div class="section-head" style="margin-top:28px">
          <h2 class="section-title"><span class="dot dot--${type}"></span>${label}广场</h2>
          <span class="muted">${total} 篇内容</span>
        </div>
        <div class="layout">
          <div>
            <div class="filter-bar">
              <button class="chip chip--active">全部</button>
              <button class="chip">最多点赞</button>
              <button class="chip">本周</button>
            </div>
            <div class="grid">${posts.length ? posts.map(postCard).join("") : emptyHTML(`还没有${label}，来当第一个吧`)}</div>
          </div>
          <aside class="sidebar">
            <div class="panel">
              <p class="kicker">关于这里</p>
              <h4>${label}角落 🌿</h4>
              <p>这里是大家分享${label}的地方。${type === "novel" ? "短篇、连载、随笔都欢迎。" : "评测、推荐、杂谈都可以。"}</p>
              <p>登录后即可发布你自己的内容。</p>
              <button class="btn btn--ghost btn--sm btn--block" id="sidePublish">+ 发布${label}</button>
            </div>
            <div class="panel">
              <p class="kicker">数据</p>
              <h4>社区小数据</h4>
              <p>📚 ${total} 篇${label}</p>
              <p>👥 ${authorCount} 位创作者</p>
              <p>❤ ${posts.reduce((s, p) => s + p.likes, 0)} 次点赞</p>
            </div>
          </aside>
        </div>
      </section>`;
  }

  function articleView(id) {
    const p = state.posts.find((x) => x.id === id);
    if (!p) return emptyHTML("找不到这篇文章", "它可能已被删除", "#/", "回到首页");
    p.views = (p.views || 0) + 1;
    db.save(state);
    const author = userById(p.authorId);
    const mine = currentUser() && currentUser().id === p.authorId;
    return `
      <article class="article view-enter">
        <a class="article-back" href="#/${p.type}">← 返回${p.type === "novel" ? "小说" : "游戏"}</a>
        <div class="article-head">
          <span class="tag tag--${p.type}">${p.type === "novel" ? "✍️ 小说" : "🎮 游戏"}</span>
          <h1 class="article-title">${escapeHtml(p.title)}</h1>
          <div class="article-byline">
            ${avatar(author.username)}
            <strong>${escapeHtml(author.username)}</strong>
            <span>· ${escapeHtml(author.bio || "这位作者很神秘")}</span>
            <span>· ${formatDate(p.createdAt)}</span>
          </div>
        </div>
        <div class="article-body">${escapeHtml(p.body)}</div>
        <div class="article-actions">
          <button class="btn btn--primary" id="likeBtn">❤ ${p.likes}</button>
          ${mine ? `<button class="btn btn--ghost" id="delBtn">删除</button>` : ""}
        </div>
      </article>`;
  }

  function publishView(presetType) {
    if (!currentUser()) {
      return `
        <section class="view-enter" style="margin-top:40px">
          ${emptyHTML("🔒 登录后才能发布", "发布内容需要一个账号（本地 Demo）", "", "")}
          <div style="text-align:center;margin-top:10px">
            <button class="btn btn--primary" id="openAuth2">登录 / 注册</button>
          </div>
        </section>`;
    }
    return `
      <section class="view-enter">
        <form class="form-card" id="publishForm">
          <h2>✍️ 写点新东西</h2>
          <p class="muted" style="margin:0 0 22px">选择类型，写上标题和正文，发布到广场。</p>

          <div class="field">
            <span>类型</span>
            <div class="type-pick">
              <div class="type-option type-option--novel ${presetType !== "game" ? "selected" : ""}" data-type="novel">
                <span class="emoji">📖</span><span class="lbl">小说</span>
                <span class="desc">故事 / 连载 / 随笔</span>
              </div>
              <div class="type-option type-option--game ${presetType === "game" ? "selected" : ""}" data-type="game">
                <span class="emoji">🎮</span><span class="lbl">游戏</span>
                <span class="desc">评测 / 推荐 / 杂谈</span>
              </div>
            </div>
          </div>

          <div class="field">
            <span>标题</span>
            <input type="text" name="title" required maxlength="60" placeholder="给作品起个名字" />
          </div>
          <div class="field">
            <span>封面 emoji（可选）</span>
            <input type="text" name="emoji" maxlength="4" placeholder="例如 📚 🏜️ 🍵" />
          </div>
          <div class="field">
            <span>一句话摘要</span>
            <input type="text" name="excerpt" required maxlength="80" placeholder="用一句话吸引读者" />
          </div>
          <div class="field">
            <span>正文</span>
            <textarea name="body" required placeholder="开始写吧…… 换行会自动保留。"></textarea>
          </div>
          <p class="muted" style="font-size:13px">提示：摘要会显示在卡片上，正文支持多段。</p>
          <div class="form-actions">
            <a class="btn btn--ghost" href="#/">取消</a>
            <button type="submit" class="btn btn--primary">发布 ✨</button>
          </div>
        </form>
      </section>`;
  }

  function emptyHTML(emojiOrTitle, sub = "", link = "", linkText = "") {
    // 第一个参数如果是单个 emoji 则当图标，否则当标题
    const isEmoji = emojiOrTitle.length <= 4;
    let icon = "🌱", title = emojiOrTitle;
    if (isEmoji) { icon = emojiOrTitle; title = sub; }
    return `
      <div class="empty">
        <span class="empty-emoji">${icon}</span>
        <h3 style="margin:0 0 6px">${escapeHtml(title)}</h3>
        ${sub && !isEmoji ? `<p style="margin:0">${escapeHtml(sub)}</p>` : ""}
        ${link && linkText ? `<a class="btn btn--ghost btn--sm" href="${link}" style="margin-top:14px">${escapeHtml(linkText)}</a>` : ""}
      </div>`;
  }

  /* ---------- 路由 ---------- */
  function router() {
    const hash = location.hash.replace(/^#/, "") || "/";
    const parts = hash.split("/").filter(Boolean); // ["novel"] / ["post","id"] ...
    view.innerHTML = "";

    let route = "home";
    if (parts.length === 0) {
      view.insertAdjacentHTML("beforeend", homeView());
      bindHome();
    } else if (parts[0] === "novel") {
      route = "novel";
      view.insertAdjacentHTML("beforeend", listView("novel"));
      bindList("novel");
    } else if (parts[0] === "game") {
      route = "game";
      view.insertAdjacentHTML("beforeend", listView("game"));
      bindList("game");
    } else if (parts[0] === "post" && parts[1]) {
      view.insertAdjacentHTML("beforeend", articleView(parts[1]));
      bindArticle(parts[1]);
    } else if (parts[0] === "publish") {
      route = "publish";
      const preset = parts[1] || "";
      view.insertAdjacentHTML("beforeend", publishView(preset));
      bindPublish();
    } else {
      view.insertAdjacentHTML("beforeend", homeView());
      bindHome();
    }
    renderChrome(route);
    bindCards();
    window.scrollTo({ top: 0, behavior: "smooth" });
  }

  /* ---------- 各视图事件绑定 ---------- */
  function bindCards() {
    $$(".card", view).forEach((c) => {
      c.addEventListener("click", () => (location.hash = "#/post/" + c.dataset.id));
    });
  }

  function bindHome() {
    const btn = $("#heroPublish");
    if (btn) btn.addEventListener("click", () => (location.hash = "#/publish"));
  }

  function bindList(type) {
    const side = $("#sidePublish");
    if (side) side.addEventListener("click", () => (location.hash = "#/publish/" + type));
    // 筛选 chip（演示用：最多点赞 / 本周）
    $$(".chip", view).forEach((chip, i) => {
      chip.addEventListener("click", () => {
        $$(".chip", view).forEach((c) => c.classList.remove("chip--active"));
        chip.classList.add("chip--active");
        let posts = postsByType(type);
        if (i === 1) posts = posts.slice().sort((a, b) => b.likes - a.likes);
        if (i === 2) {
          const wk = Date.now() - 604800000;
          posts = posts.filter((p) => p.createdAt >= wk);
        }
        const grid = $(".grid", view);
        if (grid) grid.innerHTML = posts.length ? posts.map(postCard).join("") : emptyHTML("这个筛选下还没有内容");
        bindCards();
      });
    });
  }

  function bindArticle(id) {
    const like = $("#likeBtn");
    if (like) {
      like.addEventListener("click", () => {
        const p = state.posts.find((x) => x.id === id);
        if (!p) return;
        p.likes += 1;
        db.save(state);
        like.textContent = "❤ " + p.likes;
        toast("感谢点赞 ❤");
      });
    }
    const del = $("#delBtn");
    if (del) {
      del.addEventListener("click", () => {
        state.posts = state.posts.filter((x) => x.id !== id);
        db.save(state);
        toast("已删除");
        setTimeout(() => (location.hash = "#/"), 600);
      });
    }
  }

  function bindPublish() {
    // 类型选择
    const form = $("#publishForm");
    if (!form) return;
    const open2 = $("#openAuth2");
    if (open2) open2.addEventListener("click", openAuth);
    $$(".type-option", form).forEach((opt) => {
      opt.addEventListener("click", () => {
        $$(".type-option", form).forEach((o) => o.classList.remove("selected"));
        opt.classList.add("selected");
      });
    });
    form.addEventListener("submit", (e) => {
      e.preventDefault();
      const selected = $(".type-option.selected", form);
      const type = selected ? selected.dataset.type : "novel";
      const data = new FormData(form);
      const emojiRaw = (data.get("emoji") || "").trim();
      const user = currentUser();
      const post = {
        id: uid(),
        type,
        title: (data.get("title") || "").trim(),
        emoji: emojiRaw || (type === "novel" ? "📖" : "🎮"),
        excerpt: (data.get("excerpt") || "").trim(),
        body: (data.get("body") || "").trim(),
        authorId: user.id,
        createdAt: Date.now(),
        likes: 0,
        views: 0,
      };
      state.posts.unshift(post);
      db.save(state);
      toast("发布成功！✨");
      setTimeout(() => (location.hash = "#/post/" + post.id), 600);
    });
  }

  /* ---------- 认证（模态框） ---------- */
  function openAuth() {
    const m = $("#authModal");
    m.hidden = false;
    switchTab("login");
  }
  function closeAuth() {
    $("#authModal").hidden = true;
    $("#loginHint").textContent = "";
    $("#registerHint").textContent = "";
  }
  function switchTab(which) {
    const isLogin = which === "login";
    $("#tabLogin").classList.toggle("tab--active", isLogin);
    $("#tabRegister").classList.toggle("tab--active", !isLogin);
    $("#loginForm").hidden = !isLogin;
    $("#registerForm").hidden = isLogin;
  }

  function bindAuth() {
    $("#tabLogin").addEventListener("click", () => switchTab("login"));
    $("#tabRegister").addEventListener("click", () => switchTab("register"));
    $("#authClose").addEventListener("click", closeAuth);
    $("#authModal").addEventListener("click", (e) => {
      if (e.target.id === "authModal") closeAuth();
    });
    document.addEventListener("keydown", (e) => {
      if (e.key === "Escape") closeAuth();
    });

    $("#loginForm").addEventListener("submit", (e) => {
      e.preventDefault();
      const f = new FormData(e.target);
      const u = (f.get("username") || "").trim();
      const p = f.get("password") || "";
      const found = state.users.find((x) => x.username === u && x.password === p);
      const hint = $("#loginHint");
      if (!found) {
        hint.className = "auth-hint auth-hint--err";
        hint.textContent = "用户名或密码不对哦~（试试 小墨 / 1234）";
        return;
      }
      setSession(found.id);
      hint.className = "auth-hint auth-hint--ok";
      hint.textContent = "登录成功，欢迎回来！";
      setTimeout(() => {
        closeAuth();
        renderChrome();
        router();
        toast("欢迎回来，" + found.username + " 🎉");
      }, 500);
    });

    $("#registerForm").addEventListener("submit", (e) => {
      e.preventDefault();
      const f = new FormData(e.target);
      const u = (f.get("username") || "").trim();
      const p = f.get("password") || "";
      const bio = (f.get("bio") || "").trim();
      const hint = $("#registerHint");
      if (state.users.some((x) => x.username === u)) {
        hint.className = "auth-hint auth-hint--err";
        hint.textContent = "这个名字已被占用，换一个吧~";
        return;
      }
      const user = { id: uid(), username: u, password: p, bio, createdAt: Date.now() };
      state.users.push(user);
      db.save(state);
      setSession(user.id);
      hint.className = "auth-hint auth-hint--ok";
      hint.textContent = "注册成功！已自动为你登录。";
      setTimeout(() => {
        closeAuth();
        renderChrome();
        router();
        toast("账号创建成功，开始探索吧 🌱");
      }, 500);
    });
  }

  /* ---------- 移动端菜单 ---------- */
  function bindMenu() {
    const toggle = $("#menuToggle");
    const nav = $("#nav");
    toggle.addEventListener("click", () => nav.classList.toggle("open"));
    nav.addEventListener("click", (e) => {
      if (e.target.classList.contains("nav-link")) nav.classList.remove("open");
    });
  }

  /* ---------- 启动 ---------- */
  function init() {
    bindAuth();
    bindMenu();
    window.addEventListener("hashchange", router);
    renderChrome();
    router();
  }

  // 暴露给控制台调试用（便于重置数据）
  window.MoYou = { reset: () => { state = db.reset(); router(); }, db: () => state };

  document.addEventListener("DOMContentLoaded", init);
})();
